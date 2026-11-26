// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The transaction layer (RFC 3261 section 17) over an unreliable
//! transport: the four state machines, their timers, and the matching of
//! messages to them.
//!
//! [`Transactions`] is given every message that arrives and the time, and
//! says what to send and what the layer above has to know. Retransmissions
//! are made and absorbed here: the layer above sees each request and each
//! response once.
//!
//! ```text
//!                 INVITE client            non-INVITE client
//!  request sent   Calling    A, B          Trying      E, F
//!  1xx            Proceeding               Proceeding  E, F
//!  300-699        Completed  D             Completed   K
//!  2xx            (gone)                   Completed   K
//!
//!                 INVITE server            non-INVITE server
//!  request        Proceeding               Trying
//!  1xx sent       Proceeding               Proceeding
//!  2xx sent       Accepted   L             Completed   J
//!  300-699 sent   Completed  G, H          Completed   J
//!  ACK            Confirmed  I
//! ```
//!
//! The Accepted state is the correction of RFC 6026 section 7.1: without
//! it an INVITE retransmitted after its 2xx would be taken for a new call.

use std::net::SocketAddr;

use crate::headers::Via;
use crate::message::{Essentials, Invalid, Message, Method, Request, Response};

/// T1, the round-trip estimate, in milliseconds (section 17.1.1.1).
pub const T1_MS: u64 = 500;
/// T2, the longest retransmit interval of non-INVITE requests and INVITE
/// responses.
pub const T2_MS: u64 = 4000;
/// T4, how long the network takes to clear a message.
pub const T4_MS: u64 = 5000;
/// Timers B, F, H, J and L: 64 times T1.
pub const TIMEOUT_MS: u64 = 64 * T1_MS;
/// Timer D over UDP: at least 32 seconds (section 17.1.1.2).
pub const TIMER_D_MS: u64 = 32_000;
/// What a branch starts with when it identifies a transaction (section
/// 8.1.1.7).
pub const MAGIC_COOKIE: &str = "z9hG4bK";
/// The most transactions of each kind that are kept. Requests beyond that
/// are dropped.
pub const MAX_TRANSACTIONS: usize = 4096;

/// Names a transaction to the layer above.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TxId(pub u64);

/// What the transaction layer wants done.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Output {
    /// Send this datagram.
    Send { to: SocketAddr, datagram: Vec<u8> },
    /// A request arrived and began a server transaction. Answer it with
    /// [`Transactions::respond`]. Its top Via says where it came from
    /// (`received` and `rport`, RFC 3581).
    Request {
        tx: TxId,
        from: SocketAddr,
        request: Request,
    },
    /// An ACK that belongs to no transaction: the ACK of a 2xx, which is
    /// the dialog's to take (section 13.3.1.4).
    Ack { from: SocketAddr, request: Request },
    /// A response to a request of ours.
    Response { tx: TxId, response: Response },
    /// A response that belongs to no transaction: a 2xx to an INVITE sent
    /// again, which the dialog answers with its ACK again (section 13.2.2.4).
    StrayResponse {
        from: SocketAddr,
        response: Response,
    },
    /// No final response came to a request of ours (timer B or F).
    Timeout { tx: TxId },
    /// Our final response to an INVITE was never acknowledged (timer H).
    NoAck { tx: TxId },
}

/// Why a message was not taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiveError {
    /// It lacks what a transaction is matched by.
    Invalid(Invalid),
    /// There are [`MAX_TRANSACTIONS`] server transactions already.
    Overloaded,
}

/// The address a request says it was sent from: the sent-by of the Via we
/// put on our requests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SentBy {
    pub host: String,
    pub port: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClientState {
    /// INVITE sent, nothing back.
    Calling,
    /// Non-INVITE sent, nothing back.
    Trying,
    Proceeding,
    Completed,
}

#[derive(Debug)]
struct Client {
    id: TxId,
    invite: bool,
    branch: String,
    /// The method of CSeq, which a response is matched by with the branch
    /// (section 17.1.3).
    method: String,
    state: ClientState,
    to: SocketAddr,
    request: Request,
    datagram: Vec<u8>,
    retransmit_at: Option<u64>,
    interval: u64,
    timeout_at: Option<u64>,
    /// The ACK of a 300-699, kept to answer the response when it is sent
    /// again.
    ack: Option<Vec<u8>>,
    /// A CANCEL is to follow the first provisional response.
    cancel_pending: bool,
    /// A CANCEL: the layer above is not told of its responses.
    silent: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ServerState {
    /// Non-INVITE: nothing sent yet.
    Trying,
    Proceeding,
    /// INVITE: a 2xx was sent (RFC 6026).
    Accepted,
    Completed,
    /// INVITE: the ACK of a 300-699 arrived.
    Confirmed,
}

/// What a request is matched to a server transaction by (section 17.2.3).
#[derive(Clone, Debug, PartialEq, Eq)]
struct ServerKey {
    branch: String,
    sent_by: String,
    /// The method, with ACK read as INVITE.
    method: String,
}

#[derive(Debug)]
struct Server {
    id: TxId,
    invite: bool,
    key: ServerKey,
    state: ServerState,
    reply_to: SocketAddr,
    /// The last response sent, to send again.
    last: Option<Vec<u8>>,
    retransmit_at: Option<u64>,
    interval: u64,
    timeout_at: Option<u64>,
}

/// Every transaction of one transport.
#[derive(Debug)]
pub struct Transactions {
    clients: Vec<Client>,
    servers: Vec<Server>,
    next_id: u64,
    /// What our branches are made from: a random number from the caller
    /// and a count.
    seed: u64,
    branches: u64,
}

impl Transactions {
    /// `seed` should be random: it makes our branches unique across
    /// restarts (section 8.1.1.7).
    pub fn new(seed: u64) -> Transactions {
        Transactions {
            clients: Vec::new(),
            servers: Vec::new(),
            next_id: 1,
            seed,
            branches: 0,
        }
    }

    fn id(&mut self) -> TxId {
        let id = TxId(self.next_id);
        self.next_id += 1;
        id
    }

    fn branch(&mut self) -> String {
        self.branches += 1;
        format!("{MAGIC_COOKIE}{:016x}{:x}", self.seed, self.branches)
    }

    /// Puts our Via on top of a request (section 8.1.1.7), asking for the
    /// response to come back to the port we sent from (RFC 3581 section 3).
    fn stamp(&mut self, request: &mut Request, sent_by: &SentBy) -> String {
        let branch = self.branch();
        request.headers.push_front(
            "Via",
            format!(
                "SIP/2.0/UDP {}:{};branch={branch};rport",
                sent_by.host, sent_by.port
            ),
        );
        branch
    }

    /// Sends a request in a new client transaction. The request has no Via
    /// of ours yet. An ACK is never sent this way: see
    /// [`Transactions::send_stateless`].
    pub fn send_request(
        &mut self,
        now_ms: u64,
        mut request: Request,
        to: SocketAddr,
        sent_by: &SentBy,
        out: &mut Vec<Output>,
    ) -> TxId {
        debug_assert!(request.method != Method::Ack, "an ACK has no transaction");
        let branch = self.stamp(&mut request, sent_by);
        let id = self.id();
        self.start_client(now_ms, id, branch, request, to, false, out);
        id
    }

    #[allow(clippy::too_many_arguments)]
    fn start_client(
        &mut self,
        now_ms: u64,
        id: TxId,
        branch: String,
        request: Request,
        to: SocketAddr,
        silent: bool,
        out: &mut Vec<Output>,
    ) {
        let invite = request.method == Method::Invite;
        let datagram = request.to_bytes();
        out.push(Output::Send {
            to,
            datagram: datagram.clone(),
        });
        self.clients.push(Client {
            id,
            invite,
            branch,
            method: request.method.as_str().to_owned(),
            state: if invite {
                ClientState::Calling
            } else {
                ClientState::Trying
            },
            to,
            request,
            datagram,
            // Timer A or E, and timer B or F.
            retransmit_at: Some(now_ms + T1_MS),
            interval: T1_MS,
            timeout_at: Some(now_ms + TIMEOUT_MS),
            ack: None,
            cancel_pending: false,
            silent,
        });
    }

    /// Sends a request outside any transaction, under a Via of ours with a
    /// new branch: the ACK of a 2xx (section 13.2.2.4). Returns the
    /// datagram, for the caller to send again when the 2xx comes again.
    pub fn send_stateless(
        &mut self,
        mut request: Request,
        to: SocketAddr,
        sent_by: &SentBy,
        out: &mut Vec<Output>,
    ) -> Vec<u8> {
        self.stamp(&mut request, sent_by);
        let datagram = request.to_bytes();
        out.push(Output::Send {
            to,
            datagram: datagram.clone(),
        });
        datagram
    }

    /// Cancels an INVITE of ours (section 9.1). The CANCEL goes at once if
    /// a provisional response has come, and after the first one otherwise;
    /// the INVITE then ends with whatever final response the far end
    /// gives, a 487 if the CANCEL was in time. Returns whether there was
    /// an INVITE to cancel.
    pub fn cancel(&mut self, now_ms: u64, tx: TxId, out: &mut Vec<Output>) -> bool {
        let Some(client) = self.clients.iter_mut().find(|c| c.id == tx && c.invite) else {
            return false;
        };
        match client.state {
            ClientState::Calling => {
                client.cancel_pending = true;
                true
            }
            ClientState::Proceeding => {
                client.cancel_pending = false;
                let cancel = cancel_of(&client.request);
                let branch = client.branch.clone();
                let to = client.to;
                let id = self.id();
                self.start_client(now_ms, id, branch, cancel, to, true, out);
                true
            }
            ClientState::Trying | ClientState::Completed => false,
        }
    }

    /// Answers a request. Returns whether the transaction could take the
    /// response: it cannot once a final response has been sent.
    pub fn respond(
        &mut self,
        now_ms: u64,
        tx: TxId,
        response: &Response,
        out: &mut Vec<Output>,
    ) -> bool {
        let Some(server) = self.servers.iter_mut().find(|s| s.id == tx) else {
            return false;
        };
        if !matches!(server.state, ServerState::Trying | ServerState::Proceeding) {
            return false;
        }
        let datagram = response.to_bytes();
        out.push(Output::Send {
            to: server.reply_to,
            datagram: datagram.clone(),
        });
        if response.is_provisional() {
            server.state = ServerState::Proceeding;
            server.last = Some(datagram);
        } else if server.invite && response.is_success() {
            // The 2xx is sent again by the dialog, not by us (section
            // 13.3.1.4). Timer L.
            server.state = ServerState::Accepted;
            server.last = None;
            server.timeout_at = Some(now_ms + TIMEOUT_MS);
        } else if server.invite {
            // Timers G and H.
            server.state = ServerState::Completed;
            server.last = Some(datagram);
            server.retransmit_at = Some(now_ms + T1_MS);
            server.interval = T1_MS;
            server.timeout_at = Some(now_ms + TIMEOUT_MS);
        } else {
            // Timer J.
            server.state = ServerState::Completed;
            server.last = Some(datagram);
            server.timeout_at = Some(now_ms + TIMEOUT_MS);
        }
        true
    }

    /// The INVITE a CANCEL is for, if it can still be cancelled: it has
    /// had no final response (section 9.2).
    pub fn invite_to_cancel(&self, cancel: &Request) -> Option<TxId> {
        let essentials = cancel.essentials().ok()?;
        let key = ServerKey {
            method: "INVITE".to_owned(),
            ..server_key(&essentials, &cancel.method)
        };
        self.servers
            .iter()
            .find(|s| s.key == key && s.state == ServerState::Proceeding)
            .map(|s| s.id)
    }

    /// A message arrived from `from`.
    pub fn receive(
        &mut self,
        now_ms: u64,
        from: SocketAddr,
        message: Message,
        out: &mut Vec<Output>,
    ) -> Result<(), ReceiveError> {
        match message {
            Message::Request(request) => self.receive_request(now_ms, from, request, out),
            Message::Response(response) => {
                self.receive_response(now_ms, from, response, out);
                Ok(())
            }
        }
    }

    fn receive_request(
        &mut self,
        now_ms: u64,
        from: SocketAddr,
        mut request: Request,
        out: &mut Vec<Output>,
    ) -> Result<(), ReceiveError> {
        let essentials = request.essentials().map_err(ReceiveError::Invalid)?;
        let key = server_key(&essentials, &request.method);
        let ack = request.method == Method::Ack;

        if let Some(server) = self.servers.iter_mut().find(|s| s.key == key) {
            if ack {
                // Section 17.2.1: the ACK of a 300-699 stops the response
                // being sent again. Timer I.
                if server.state == ServerState::Completed {
                    server.state = ServerState::Confirmed;
                    server.retransmit_at = None;
                    server.timeout_at = Some(now_ms + T4_MS);
                }
            } else if let Some(last) = &server.last
                && matches!(
                    server.state,
                    ServerState::Proceeding | ServerState::Completed
                )
            {
                // The request again: the last response again.
                out.push(Output::Send {
                    to: server.reply_to,
                    datagram: last.clone(),
                });
            }
            return Ok(());
        }

        if ack {
            out.push(Output::Ack { from, request });
            return Ok(());
        }
        if self.servers.len() >= MAX_TRANSACTIONS {
            return Err(ReceiveError::Overloaded);
        }

        // Section 18.2.2 and RFC 3581 section 4: the response goes back to
        // where the request came from when the Via asks for that, and to
        // the address it came from at the port the Via names otherwise.
        let mut via = essentials.via;
        let reply_to = if via.params.has("rport") {
            via.params.set("received", Some(&from.ip().to_string()));
            via.params.set("rport", Some(&from.port().to_string()));
            from
        } else {
            if via.host != from.ip().to_string() {
                via.params.set("received", Some(&from.ip().to_string()));
            }
            SocketAddr::new(from.ip(), via.port_or_default())
        };
        request.headers.replace_top_via(&via);

        let id = self.id();
        let invite = request.method == Method::Invite;
        self.servers.push(Server {
            id,
            invite,
            key,
            state: if invite {
                ServerState::Proceeding
            } else {
                ServerState::Trying
            },
            reply_to,
            last: None,
            retransmit_at: None,
            interval: T1_MS,
            timeout_at: None,
        });
        out.push(Output::Request {
            tx: id,
            from,
            request,
        });
        Ok(())
    }

    fn receive_response(
        &mut self,
        now_ms: u64,
        from: SocketAddr,
        response: Response,
        out: &mut Vec<Output>,
    ) {
        // Section 17.1.3: the branch of the top Via and the method of CSeq.
        let matched = response.headers.top_via().and_then(|via| {
            let branch = via.branch()?.to_owned();
            let method = response.headers.cseq()?.method;
            self.clients
                .iter()
                .position(|c| c.branch == branch && c.method == method)
        });
        let Some(index) = matched else {
            out.push(Output::StrayResponse { from, response });
            return;
        };
        let client = &mut self.clients[index];
        let provisional = response.is_provisional();

        let (pass, done) = match (client.invite, client.state) {
            (true, ClientState::Calling | ClientState::Proceeding) => {
                if provisional {
                    // Section 17.1.1.2: timers A and B stop.
                    client.state = ClientState::Proceeding;
                    client.retransmit_at = None;
                    client.timeout_at = None;
                    (true, false)
                } else if response.is_success() {
                    (true, true)
                } else {
                    // Section 17.1.1.3: the transaction sends the ACK of a
                    // 300-699 itself. Timer D.
                    let ack = ack_of(&client.request, &response).to_bytes();
                    out.push(Output::Send {
                        to: client.to,
                        datagram: ack.clone(),
                    });
                    client.ack = Some(ack);
                    client.state = ClientState::Completed;
                    client.retransmit_at = None;
                    client.timeout_at = Some(now_ms + TIMER_D_MS);
                    (true, false)
                }
            }
            (true, _) => {
                // Completed: a final response again gets the ACK again.
                if !provisional
                    && !response.is_success()
                    && let Some(ack) = &client.ack
                {
                    out.push(Output::Send {
                        to: client.to,
                        datagram: ack.clone(),
                    });
                }
                (false, false)
            }
            (false, ClientState::Trying | ClientState::Proceeding) => {
                if provisional {
                    // Section 17.1.2.2: timer E goes on, at T2.
                    client.state = ClientState::Proceeding;
                    (true, false)
                } else {
                    // Timer K.
                    client.state = ClientState::Completed;
                    client.retransmit_at = None;
                    client.timeout_at = Some(now_ms + T4_MS);
                    (true, false)
                }
            }
            (false, _) => (false, false),
        };

        let id = client.id;
        let silent = client.silent;
        let cancel = provisional && client.invite && client.cancel_pending;
        if done {
            self.clients.swap_remove(index);
        }
        if cancel {
            self.cancel(now_ms, id, out);
        }
        if pass && !silent {
            out.push(Output::Response { tx: id, response });
        }
    }

    /// Runs the timers that are due.
    pub fn poll(&mut self, now_ms: u64, out: &mut Vec<Output>) {
        self.clients.retain_mut(|client| {
            if client.timeout_at.is_some_and(|at| now_ms >= at) {
                // Timer B or F with no final response; timer D or K after
                // one.
                if client.state != ClientState::Completed && !client.silent {
                    out.push(Output::Timeout { tx: client.id });
                }
                return false;
            }
            if client.retransmit_at.is_some_and(|at| now_ms >= at) {
                out.push(Output::Send {
                    to: client.to,
                    datagram: client.datagram.clone(),
                });
                // Timer A doubles without limit; timer E doubles up to T2
                // and is T2 once a provisional response has come.
                client.interval = if client.invite {
                    client.interval * 2
                } else if client.state == ClientState::Proceeding {
                    T2_MS
                } else {
                    (client.interval * 2).min(T2_MS)
                };
                client.retransmit_at = Some(now_ms + client.interval);
            }
            true
        });
        self.servers.retain_mut(|server| {
            if server.timeout_at.is_some_and(|at| now_ms >= at) {
                // Timer H ends a transaction whose response was never
                // acknowledged; I, J and L end one that is done.
                if server.invite && server.state == ServerState::Completed {
                    out.push(Output::NoAck { tx: server.id });
                }
                return false;
            }
            if server.retransmit_at.is_some_and(|at| now_ms >= at)
                && let Some(last) = &server.last
            {
                // Timer G.
                out.push(Output::Send {
                    to: server.reply_to,
                    datagram: last.clone(),
                });
                server.interval = (server.interval * 2).min(T2_MS);
                server.retransmit_at = Some(now_ms + server.interval);
            }
            true
        });
    }

    /// When [`Transactions::poll`] next has something to do.
    pub fn next_deadline(&self) -> Option<u64> {
        let clients = self
            .clients
            .iter()
            .flat_map(|c| [c.retransmit_at, c.timeout_at]);
        let servers = self
            .servers
            .iter()
            .flat_map(|s| [s.retransmit_at, s.timeout_at]);
        clients.chain(servers).flatten().min()
    }

    /// How many transactions are alive: client, then server.
    pub fn len(&self) -> (usize, usize) {
        (self.clients.len(), self.servers.len())
    }

    pub fn is_empty(&self) -> bool {
        self.clients.is_empty() && self.servers.is_empty()
    }

    /// Ends a server transaction that will get no response: its request
    /// was dropped on purpose.
    pub fn forget_server(&mut self, tx: TxId) {
        self.servers.retain(|s| s.id != tx);
    }
}

/// The key of the server transaction a request belongs to (section
/// 17.2.3). A branch without the magic cookie is from an RFC 2543 element
/// and is not unique: the Call-ID, the CSeq number and the From tag stand
/// in for it.
fn server_key(essentials: &Essentials, method: &Method) -> ServerKey {
    let via: &Via = &essentials.via;
    let branch = match via.branch() {
        Some(branch) if branch.starts_with(MAGIC_COOKIE) => branch.to_owned(),
        other => format!(
            "rfc2543;{};{};{};{}",
            essentials.call_id,
            essentials.cseq.sequence,
            essentials.from.tag().unwrap_or(""),
            other.unwrap_or("")
        ),
    };
    ServerKey {
        branch,
        sent_by: format!(
            "{}:{}",
            via.host.to_ascii_lowercase(),
            via.port_or_default()
        ),
        method: if *method == Method::Ack {
            "INVITE".to_owned()
        } else {
            method.as_str().to_owned()
        },
    }
}

/// A request made from another: the same Request-URI, top Via, From,
/// Call-ID, Route and CSeq number.
fn derived(original: &Request, method: Method, to: &str) -> Request {
    let mut request = Request {
        method,
        uri: original.uri.clone(),
        headers: crate::headers::Headers::new(),
        body: Vec::new(),
    };
    let headers = &mut request.headers;
    // Our own Via is the first of the original and stands alone.
    if let Some(via) = original.headers.get("Via") {
        headers.push("Via", via);
    }
    headers.push("Max-Forwards", "70");
    if let Some(from) = original.headers.get("From") {
        headers.push("From", from);
    }
    headers.push("To", to);
    if let Some(call_id) = original.headers.get("Call-ID") {
        headers.push("Call-ID", call_id);
    }
    let sequence = original.headers.cseq().map_or(0, |cseq| cseq.sequence);
    headers.push("CSeq", format!("{sequence} {}", request.method));
    for route in original.headers.all("Route") {
        headers.push("Route", route);
    }
    request
}

/// The ACK of a 300-699 (section 17.1.1.3): the To of the response, the
/// rest from the INVITE.
fn ack_of(invite: &Request, response: &Response) -> Request {
    let to = response
        .headers
        .get("To")
        .or(invite.headers.get("To"))
        .unwrap_or_default();
    derived(invite, Method::Ack, to)
}

/// The CANCEL of an INVITE (section 9.1): the To of the INVITE, with no
/// tag, and the same branch.
fn cancel_of(invite: &Request) -> Request {
    let to = invite.headers.get("To").unwrap_or_default();
    derived(invite, Method::Cancel, to)
}
