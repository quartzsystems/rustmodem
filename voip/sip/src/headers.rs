// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Header fields (RFC 3261 section 20) and typed access to the ones a user
//! agent reads.
//!
//! Headers are kept as they arrived: name and value, in order, unknown ones
//! included. The typed getters parse on demand, and answer `None` when the
//! header is missing or is not what its grammar says.

use std::fmt;

use crate::text::{Params, find_unquoted, is_token, quote, split_top, unquote, unquote_or_token};
use crate::uri::{SipUri, UriError, split_host_port};

/// The headers with a compact form (RFC 3261 section 7.3.3), and the ones
/// this crate reads, in the spelling they are written in.
const KNOWN: &[(&str, &str)] = &[
    ("Via", "v"),
    ("From", "f"),
    ("To", "t"),
    ("Call-ID", "i"),
    ("Contact", "m"),
    ("Content-Type", "c"),
    ("Content-Length", "l"),
    ("Content-Encoding", "e"),
    ("Subject", "s"),
    ("Supported", "k"),
    ("CSeq", ""),
    ("Max-Forwards", ""),
    ("Route", ""),
    ("Record-Route", ""),
    ("Expires", ""),
    ("Min-Expires", ""),
    ("Allow", ""),
    ("Require", ""),
    ("Unsupported", ""),
    ("User-Agent", ""),
    ("Server", ""),
    ("WWW-Authenticate", ""),
    ("Proxy-Authenticate", ""),
    ("Authorization", ""),
    ("Proxy-Authorization", ""),
];

/// The name a header is kept under: the long form of a compact name and
/// the usual spelling of a known one. Any other name is kept as written.
pub(crate) fn canonical(name: &str) -> String {
    KNOWN
        .iter()
        .find(|(long, compact)| {
            long.eq_ignore_ascii_case(name)
                || (!compact.is_empty() && compact.eq_ignore_ascii_case(name))
        })
        .map_or_else(|| name.to_owned(), |(long, _)| (*long).to_owned())
}

/// One header field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub name: String,
    pub value: String,
}

/// The header fields of a message, in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Headers(pub Vec<Header>);

impl Headers {
    pub fn new() -> Headers {
        Headers(Vec::new())
    }

    /// Adds a header after the others.
    pub fn push(&mut self, name: &str, value: impl Into<String>) {
        self.0.push(Header {
            name: canonical(name),
            value: value.into(),
        });
    }

    /// Adds a header before the others: a Via goes on top.
    pub fn push_front(&mut self, name: &str, value: impl Into<String>) {
        self.0.insert(
            0,
            Header {
                name: canonical(name),
                value: value.into(),
            },
        );
    }

    /// Replaces every header of the name with one, where the first stood,
    /// or adds it.
    pub fn set(&mut self, name: &str, value: impl Into<String>) {
        let name = canonical(name);
        let at = self
            .0
            .iter()
            .position(|h| h.name.eq_ignore_ascii_case(&name));
        self.0.retain(|h| !h.name.eq_ignore_ascii_case(&name));
        let header = Header {
            name,
            value: value.into(),
        };
        match at {
            Some(at) => self.0.insert(at.min(self.0.len()), header),
            None => self.0.push(header),
        }
    }

    pub fn remove(&mut self, name: &str) {
        let name = canonical(name);
        self.0.retain(|h| !h.name.eq_ignore_ascii_case(&name));
    }

    /// The value of the first header of the name.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.all(name).next()
    }

    /// The values of every header of the name, in order.
    pub fn all<'a>(&'a self, name: &str) -> impl Iterator<Item = &'a str> + use<'a> {
        let name = canonical(name);
        self.0
            .iter()
            .filter(move |h| h.name.eq_ignore_ascii_case(&name))
            .map(|h| h.value.as_str())
    }

    pub fn has(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    /// Writes `via` in place of the topmost Via and leaves the ones under
    /// it as they are. Nothing happens when there is no Via.
    pub fn replace_top_via(&mut self, via: &Via) {
        let Some(header) = self
            .0
            .iter_mut()
            .find(|h| h.name.eq_ignore_ascii_case("Via") && !h.value.trim().is_empty())
        else {
            return;
        };
        let mut elements = split_top(&header.value, ',')
            .into_iter()
            .map(str::trim)
            .filter(|element| !element.is_empty());
        elements.next();
        let mut value = via.to_string();
        for element in elements {
            value.push_str(", ");
            value.push_str(element);
        }
        header.value = value;
    }

    /// Every element of a header that may be given several times or as a
    /// comma-separated list (RFC 3261 section 7.3.1), in order.
    fn elements(&self, name: &str) -> Vec<&str> {
        self.all(name)
            .flat_map(|value| split_top(value, ','))
            .map(str::trim)
            .filter(|element| !element.is_empty())
            .collect()
    }

    /// Every Via, topmost first. `None` when one of them is not valid.
    pub fn vias(&self) -> Option<Vec<Via>> {
        self.elements("Via").into_iter().map(Via::parse).collect()
    }

    /// The topmost Via.
    pub fn top_via(&self) -> Option<Via> {
        self.elements("Via").first().and_then(|via| Via::parse(via))
    }

    pub fn from(&self) -> Option<NameAddr> {
        self.get("From").and_then(NameAddr::parse)
    }

    pub fn to(&self) -> Option<NameAddr> {
        self.get("To").and_then(NameAddr::parse)
    }

    pub fn call_id(&self) -> Option<&str> {
        self.get("Call-ID")
            .map(str::trim)
            .filter(|id| !id.is_empty() && !id.bytes().any(|b| b.is_ascii_whitespace()))
    }

    pub fn cseq(&self) -> Option<CSeq> {
        self.get("CSeq").and_then(CSeq::parse)
    }

    /// The Contact addresses. `None` when one is not valid; `*` is given
    /// as [`Contacts::Star`].
    pub fn contacts(&self) -> Option<Contacts> {
        let elements = self.elements("Contact");
        if elements == ["*"] {
            return Some(Contacts::Star);
        }
        elements
            .into_iter()
            .map(NameAddr::parse)
            .collect::<Option<Vec<_>>>()
            .map(Contacts::List)
    }

    /// The first Contact address.
    pub fn contact(&self) -> Option<NameAddr> {
        match self.contacts()? {
            Contacts::List(list) => list.into_iter().next(),
            Contacts::Star => None,
        }
    }

    pub fn max_forwards(&self) -> Option<u32> {
        self.get("Max-Forwards").and_then(number)
    }

    pub fn expires(&self) -> Option<u32> {
        self.get("Expires").and_then(number)
    }

    pub fn min_expires(&self) -> Option<u32> {
        self.get("Min-Expires").and_then(number)
    }

    /// The Route addresses, in order. `None` when one is not valid.
    pub fn routes(&self) -> Option<Vec<NameAddr>> {
        self.elements("Route")
            .into_iter()
            .map(NameAddr::parse)
            .collect()
    }

    /// The Record-Route addresses, in order. `None` when one is not valid.
    pub fn record_routes(&self) -> Option<Vec<NameAddr>> {
        self.elements("Record-Route")
            .into_iter()
            .map(NameAddr::parse)
            .collect()
    }

    /// The methods of Allow.
    pub fn allow(&self) -> Vec<&str> {
        self.elements("Allow")
    }

    /// The option tags of Supported.
    pub fn supported(&self) -> Vec<&str> {
        self.elements("Supported")
    }

    /// The option tags of Require.
    pub fn require(&self) -> Vec<&str> {
        self.elements("Require")
    }

    pub fn user_agent(&self) -> Option<&str> {
        self.get("User-Agent")
    }

    /// The media type, without its parameters, in lower case.
    pub fn content_type(&self) -> Option<String> {
        let value = self.get("Content-Type")?;
        let media = value.split(';').next()?.trim();
        (!media.is_empty()).then(|| media.to_ascii_lowercase())
    }

    /// The challenges of WWW-Authenticate, one a header line.
    pub fn www_authenticate(&self) -> Vec<Auth> {
        self.all("WWW-Authenticate")
            .filter_map(Auth::parse)
            .collect()
    }

    /// The challenges of Proxy-Authenticate, one a header line.
    pub fn proxy_authenticate(&self) -> Vec<Auth> {
        self.all("Proxy-Authenticate")
            .filter_map(Auth::parse)
            .collect()
    }

    pub fn authorization(&self) -> Vec<Auth> {
        self.all("Authorization").filter_map(Auth::parse).collect()
    }

    pub fn proxy_authorization(&self) -> Vec<Auth> {
        self.all("Proxy-Authorization")
            .filter_map(Auth::parse)
            .collect()
    }
}

/// A decimal number of digits only, as the numeric headers are.
fn number(text: &str) -> Option<u32> {
    let text = text.trim();
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// One Via (RFC 3261 section 20.42): `SIP/2.0/UDP host:port;params`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Via {
    /// `UDP`, as written.
    pub transport: String,
    pub host: String,
    pub port: Option<u16>,
    pub params: Params,
}

impl Via {
    pub fn parse(text: &str) -> Option<Via> {
        let (sent, params) = match text.split_once(';') {
            Some((sent, params)) => (sent, Params::parse(params)?),
            None => (text, Params::new()),
        };
        // White space is allowed around the slashes (section 7.3.1).
        let mut parts = sent.splitn(3, '/');
        let name = parts.next()?.trim();
        let version = parts.next()?.trim();
        let rest = parts.next()?.trim();
        if !name.eq_ignore_ascii_case("SIP") || version != "2.0" {
            return None;
        }
        let (transport, sent_by) = rest.split_once(|c: char| c.is_ascii_whitespace())?;
        if !is_token(transport) {
            return None;
        }
        let sent_by: String = sent_by.split_ascii_whitespace().collect();
        let (host, port) = split_host_port(&sent_by).ok()?;
        Some(Via {
            transport: transport.to_owned(),
            host: host.to_owned(),
            port,
            params,
        })
    }

    /// The branch parameter: the transaction id (section 8.1.1.7).
    pub fn branch(&self) -> Option<&str> {
        self.params.get("branch")
    }

    /// The port in the sent-by, or 5060.
    pub fn port_or_default(&self) -> u16 {
        self.port.unwrap_or(5060)
    }
}

impl fmt::Display for Via {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SIP/2.0/{} {}", self.transport, self.host)?;
        if let Some(port) = self.port {
            write!(f, ":{port}")?;
        }
        write!(f, "{}", self.params)
    }
}

/// An address with an optional display name and parameters, as in From,
/// To, Contact, Route and Record-Route (RFC 3261 section 20.10).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameAddr {
    /// The display name, without quotes.
    pub display: Option<String>,
    /// The URI as written. It is not always a `sip:` one.
    pub uri: String,
    /// The header parameters: `tag`, `expires`, `q` and so on.
    pub params: Params,
}

impl NameAddr {
    /// `<uri>`.
    pub fn new(uri: &SipUri) -> NameAddr {
        NameAddr {
            display: None,
            uri: uri.to_string(),
            params: Params::new(),
        }
    }

    pub fn parse(text: &str) -> Option<NameAddr> {
        let text = text.trim();
        let (display, uri, params) = match find_unquoted(text, '<') {
            Some(open) => {
                let close = open + text[open..].find('>')?;
                let display = text[..open].trim();
                let display = if display.is_empty() {
                    None
                } else if display.starts_with('"') {
                    Some(unquote(display)?)
                } else {
                    Some(
                        display
                            .split_ascii_whitespace()
                            .collect::<Vec<_>>()
                            .join(" "),
                    )
                };
                let after = text[close + 1..].trim();
                if !after.is_empty() && !after.starts_with(';') {
                    return None;
                }
                (display, text[open + 1..close].trim(), after)
            }
            // Without brackets everything after the first semicolon is a
            // parameter of the header, not of the URI (section 20.10).
            None => match text.split_once(';') {
                Some((uri, params)) => (None, uri.trim(), params),
                None => (None, text, ""),
            },
        };
        if uri.is_empty() || uri.bytes().any(|b| b <= b' ' || b == b'"') {
            return None;
        }
        Some(NameAddr {
            display,
            uri: uri.to_owned(),
            params: Params::parse(params)?,
        })
    }

    /// The URI as a `sip:` one.
    pub fn sip_uri(&self) -> Result<SipUri, UriError> {
        SipUri::parse(&self.uri)
    }

    pub fn tag(&self) -> Option<&str> {
        self.params.get("tag")
    }
}

impl fmt::Display for NameAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(display) = &self.display {
            write!(f, "{} ", quote(display))?;
        }
        write!(f, "<{}>{}", self.uri, self.params)
    }
}

/// What Contact holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Contacts {
    /// `Contact: *`, which removes every binding (section 10.2.2).
    Star,
    List(Vec<NameAddr>),
}

/// The CSeq header (RFC 3261 section 20.16).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CSeq {
    pub sequence: u32,
    pub method: String,
}

impl CSeq {
    pub fn parse(text: &str) -> Option<CSeq> {
        let mut parts = text.split_ascii_whitespace();
        let sequence = number(parts.next()?)?;
        let method = parts.next()?;
        // Section 8.1.1.5: the sequence number is less than 2^31.
        if parts.next().is_some() || !is_token(method) || sequence > i32::MAX as u32 {
            return None;
        }
        Some(CSeq {
            sequence,
            method: method.to_owned(),
        })
    }
}

impl fmt::Display for CSeq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.sequence, self.method)
    }
}

/// A challenge or a set of credentials: a scheme and its parameters, with
/// the quotes taken off the values (RFC 3261 sections 20.7 and 20.44).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Auth {
    /// `Digest`, as written.
    pub scheme: String,
    pub params: Vec<(String, String)>,
}

impl Auth {
    pub fn parse(text: &str) -> Option<Auth> {
        let text = text.trim();
        let (scheme, rest) = text.split_once(|c: char| c.is_ascii_whitespace())?;
        if !is_token(scheme) {
            return None;
        }
        let mut params = Vec::new();
        for part in split_top(rest, ',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let (name, value) = part.split_once('=')?;
            let name = name.trim();
            if !is_token(name) {
                return None;
            }
            params.push((name.to_owned(), unquote_or_token(value.trim())?));
        }
        Some(Auth {
            scheme: scheme.to_owned(),
            params,
        })
    }

    /// The value of a parameter; names are compared without regard to case.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub fn is_digest(&self) -> bool {
        self.scheme.eq_ignore_ascii_case("Digest")
    }
}
