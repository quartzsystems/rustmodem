// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! SIP messages (RFC 3261 section 7): parse and build.
//!
//! The parser reads one datagram. It never panics and never allocates in
//! proportion to a number the message states: a `Content-Length` larger
//! than the datagram is an error, not a buffer.

use std::fmt;

use crate::headers::{CSeq, Header, Headers, NameAddr, Via, canonical};
use crate::text::is_token;
use crate::uri::{SipUri, UriError};

/// The longest datagram that is read: the most UDP can carry.
pub const MAX_DATAGRAM: usize = 65_535;

/// The most header fields a message may have.
pub const MAX_HEADERS: usize = 128;

/// The longest line, in octets, a message may have. A folded header is one
/// line for each line it was sent as.
pub const MAX_LINE: usize = 8192;

/// A request method (RFC 3261 section 7.1). Method names are case
/// sensitive.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Method {
    Invite,
    Ack,
    Bye,
    Cancel,
    Options,
    Register,
    /// Any other token.
    Other(String),
}

impl Method {
    pub fn parse(text: &str) -> Option<Method> {
        Some(match text {
            "INVITE" => Method::Invite,
            "ACK" => Method::Ack,
            "BYE" => Method::Bye,
            "CANCEL" => Method::Cancel,
            "OPTIONS" => Method::Options,
            "REGISTER" => Method::Register,
            other if is_token(other) => Method::Other(other.to_owned()),
            _ => return None,
        })
    }

    pub fn as_str(&self) -> &str {
        match self {
            Method::Invite => "INVITE",
            Method::Ack => "ACK",
            Method::Bye => "BYE",
            Method::Cancel => "CANCEL",
            Method::Options => "OPTIONS",
            Method::Register => "REGISTER",
            Method::Other(other) => other,
        }
    }
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub method: Method,
    /// The Request-URI as written. It is not always a `sip:` one; see
    /// [`Request::sip_uri`].
    pub uri: String,
    pub headers: Headers,
    pub body: Vec<u8>,
}

/// A response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    /// 100 to 699.
    pub status: u16,
    pub reason: String,
    pub headers: Headers,
    pub body: Vec<u8>,
}

/// A request or a response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    Request(Request),
    Response(Response),
}

/// Why a datagram is not a SIP message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// Longer than [`MAX_DATAGRAM`].
    TooLong,
    /// A line longer than [`MAX_LINE`].
    LineTooLong,
    /// More than [`MAX_HEADERS`] header fields.
    TooManyHeaders,
    /// The start line and the headers are not UTF-8.
    NotUtf8,
    /// Nothing but line ends, as a keep-alive is.
    Empty,
    /// The first line is neither a Request-Line nor a Status-Line.
    BadStartLine,
    /// The version is not `SIP/2.0`.
    BadVersion,
    /// A header line with no colon, or with a name that is not a token.
    BadHeader,
    /// There is no empty line after the headers.
    NoEndOfHeaders,
    /// `Content-Length` is not a number, or is given twice with two values.
    BadContentLength,
    /// The body is shorter than `Content-Length` says.
    Truncated,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ParseError::TooLong => "the message is too long",
            ParseError::LineTooLong => "a line of the message is too long",
            ParseError::TooManyHeaders => "the message has too many headers",
            ParseError::NotUtf8 => "the message is not text",
            ParseError::Empty => "the message is empty",
            ParseError::BadStartLine => "the first line is not a request or a status line",
            ParseError::BadVersion => "the SIP version is not 2.0",
            ParseError::BadHeader => "a header line is not valid",
            ParseError::NoEndOfHeaders => "the headers do not end",
            ParseError::BadContentLength => "Content-Length is not valid",
            ParseError::Truncated => "the body is shorter than Content-Length",
        })
    }
}

impl std::error::Error for ParseError {}

/// What a request or a response must have to be handled at all (RFC 3261
/// section 8.1.1): the top Via, From, To, Call-ID and CSeq.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Essentials {
    pub via: Via,
    pub from: NameAddr,
    pub to: NameAddr,
    pub call_id: String,
    pub cseq: CSeq,
}

/// Why a message that parsed cannot be handled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Invalid {
    /// A required header is missing or is not valid. The name is the
    /// header's.
    Header(&'static str),
    /// The method of CSeq is not the method of the request (section
    /// 8.2.6.2 has them equal).
    CSeqMethod,
}

impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Invalid::Header(name) => write!(f, "the {name} header is missing or not valid"),
            Invalid::CSeqMethod => write!(f, "the CSeq method is not the request method"),
        }
    }
}

impl std::error::Error for Invalid {}

fn essentials(headers: &Headers) -> Result<Essentials, Invalid> {
    // Every Via has to be valid for a response to find its way back.
    let via = headers
        .vias()
        .and_then(|vias| vias.into_iter().next())
        .ok_or(Invalid::Header("Via"))?;
    Ok(Essentials {
        via,
        from: headers.from().ok_or(Invalid::Header("From"))?,
        to: headers.to().ok_or(Invalid::Header("To"))?,
        call_id: headers
            .call_id()
            .ok_or(Invalid::Header("Call-ID"))?
            .to_owned(),
        cseq: headers.cseq().ok_or(Invalid::Header("CSeq"))?,
    })
}

impl Request {
    /// A request with no headers and no body.
    pub fn new(method: Method, uri: &SipUri) -> Request {
        Request {
            method,
            uri: uri.to_string(),
            headers: Headers::new(),
            body: Vec::new(),
        }
    }

    /// The Request-URI as a `sip:` URI.
    pub fn sip_uri(&self) -> Result<SipUri, UriError> {
        SipUri::parse(&self.uri)
    }

    pub fn essentials(&self) -> Result<Essentials, Invalid> {
        let essentials = essentials(&self.headers)?;
        if essentials.cseq.method != self.method.as_str() {
            return Err(Invalid::CSeqMethod);
        }
        Ok(essentials)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let start = format!("{} {} SIP/2.0", self.method, self.uri);
        build(&start, &self.headers, &self.body)
    }
}

impl Response {
    /// A response with no headers and no body, with the usual reason
    /// phrase for the status.
    pub fn new(status: u16) -> Response {
        Response {
            status,
            reason: reason_phrase(status).to_owned(),
            headers: Headers::new(),
            body: Vec::new(),
        }
    }

    pub fn essentials(&self) -> Result<Essentials, Invalid> {
        essentials(&self.headers)
    }

    /// 100 to 199.
    pub fn is_provisional(&self) -> bool {
        self.status < 200
    }

    /// 200 to 299.
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let start = format!("SIP/2.0 {} {}", self.status, self.reason);
        build(&start, &self.headers, &self.body)
    }
}

impl Message {
    pub fn parse(datagram: &[u8]) -> Result<Message, ParseError> {
        parse(datagram)
    }

    pub fn headers(&self) -> &Headers {
        match self {
            Message::Request(request) => &request.headers,
            Message::Response(response) => &response.headers,
        }
    }

    pub fn body(&self) -> &[u8] {
        match self {
            Message::Request(request) => &request.body,
            Message::Response(response) => &response.body,
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            Message::Request(request) => request.to_bytes(),
            Message::Response(response) => response.to_bytes(),
        }
    }
}

/// The reason phrase that goes with a status code (RFC 3261 section 21).
pub fn reason_phrase(status: u16) -> &'static str {
    match status {
        100 => "Trying",
        180 => "Ringing",
        183 => "Session Progress",
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        407 => "Proxy Authentication Required",
        408 => "Request Timeout",
        415 => "Unsupported Media Type",
        416 => "Unsupported URI Scheme",
        420 => "Bad Extension",
        423 => "Interval Too Brief",
        480 => "Temporarily Unavailable",
        481 => "Call/Transaction Does Not Exist",
        482 => "Loop Detected",
        483 => "Too Many Hops",
        486 => "Busy Here",
        487 => "Request Terminated",
        488 => "Not Acceptable Here",
        491 => "Request Pending",
        500 => "Server Internal Error",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        504 => "Server Time-out",
        600 => "Busy Everywhere",
        603 => "Decline",
        604 => "Does Not Exist Anywhere",
        606 => "Not Acceptable",
        _ => match status / 100 {
            1 => "Progress",
            2 => "OK",
            3 => "Redirect",
            4 => "Request Failure",
            5 => "Server Failure",
            _ => "Global Failure",
        },
    }
}

/// The start line, the headers, a `Content-Length` that is always the
/// length of the body, an empty line, and the body.
fn build(start: &str, headers: &Headers, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(512 + body.len());
    push_line(&mut out, start);
    for header in &headers.0 {
        if header.name.eq_ignore_ascii_case("Content-Length") {
            continue;
        }
        push_line(&mut out, &format!("{}: {}", header.name, header.value));
    }
    push_line(&mut out, &format!("Content-Length: {}", body.len()));
    out.extend_from_slice(b"\r\n");
    out.extend_from_slice(body);
    out
}

/// One line. Nothing in it can end the line early: a line break in a
/// value is written as a space.
fn push_line(out: &mut Vec<u8>, line: &str) {
    out.extend(
        line.bytes()
            .map(|b| if matches!(b, b'\r' | b'\n') { b' ' } else { b }),
    );
    out.extend_from_slice(b"\r\n");
}

fn parse(datagram: &[u8]) -> Result<Message, ParseError> {
    if datagram.len() > MAX_DATAGRAM {
        return Err(ParseError::TooLong);
    }
    // Section 7.5: line ends before the start line are ignored.
    let skip = datagram
        .iter()
        .position(|b| !matches!(b, b'\r' | b'\n'))
        .ok_or(ParseError::Empty)?;
    let mut rest = &datagram[skip..];

    // The lines up to the empty one. A line ends in CRLF; a bare LF is
    // taken too.
    let mut lines: Vec<&str> = Vec::new();
    loop {
        let end = rest
            .iter()
            .position(|b| *b == b'\n')
            .ok_or(ParseError::NoEndOfHeaders)?;
        let line = &rest[..end];
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        rest = &rest[end + 1..];
        if line.is_empty() {
            break;
        }
        if line.len() > MAX_LINE {
            return Err(ParseError::LineTooLong);
        }
        // A carriage return that does not end a line is not text.
        if line.contains(&b'\r') {
            return Err(ParseError::BadHeader);
        }
        // Headers are counted when they are put together; this bounds the
        // lines they may be folded over.
        if lines.len() > 4 * MAX_HEADERS {
            return Err(ParseError::TooManyHeaders);
        }
        lines.push(std::str::from_utf8(line).map_err(|_| ParseError::NotUtf8)?);
    }
    let body = rest;

    let mut lines = lines.into_iter();
    let start = lines.next().ok_or(ParseError::BadStartLine)?;

    // Section 7.3.1: a line that starts with white space continues the
    // header before it, and the fold reads as one space.
    let mut headers = Headers::new();
    let mut content_length: Option<usize> = None;
    let mut pending: Option<(String, String)> = None;
    let mut finish =
        |header: Option<(String, String)>, headers: &mut Headers| -> Result<(), ParseError> {
            let Some((name, value)) = header else {
                return Ok(());
            };
            let name = canonical(&name);
            let value = value.trim();
            if name == "Content-Length" {
                let length = value
                    .bytes()
                    .all(|b| b.is_ascii_digit())
                    .then(|| value.parse::<usize>().ok())
                    .flatten()
                    .ok_or(ParseError::BadContentLength)?;
                if content_length.is_some_and(|known| known != length) {
                    return Err(ParseError::BadContentLength);
                }
                content_length = Some(length);
                return Ok(());
            }
            if headers.0.len() >= MAX_HEADERS {
                return Err(ParseError::TooManyHeaders);
            }
            // A folded header is one line when it is written again.
            if name.len() + 2 + value.len() > MAX_LINE {
                return Err(ParseError::LineTooLong);
            }
            headers.0.push(Header {
                name,
                value: value.to_owned(),
            });
            Ok(())
        };
    for line in lines {
        if line.starts_with([' ', '\t']) {
            let (_, value) = pending.as_mut().ok_or(ParseError::BadHeader)?;
            value.push(' ');
            value.push_str(line.trim());
            continue;
        }
        finish(pending.take(), &mut headers)?;
        let (name, value) = line.split_once(':').ok_or(ParseError::BadHeader)?;
        let name = name.trim_end();
        if !is_token(name) {
            return Err(ParseError::BadHeader);
        }
        pending = Some((name.to_owned(), value.trim().to_owned()));
    }
    finish(pending.take(), &mut headers)?;

    // Section 18.3: over UDP, octets after the body are dropped and a body
    // that is short is an error. Without Content-Length the body runs to
    // the end of the datagram.
    let body = match content_length {
        Some(length) => body.get(..length).ok_or(ParseError::Truncated)?,
        None => body,
    }
    .to_vec();

    start_line(start, headers, body)
}

fn start_line(start: &str, headers: Headers, body: Vec<u8>) -> Result<Message, ParseError> {
    // Section 7.2: "SIP/2.0 SP Status-Code SP Reason-Phrase".
    if start
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("SIP/"))
    {
        let mut parts = start.splitn(3, ' ');
        let version = parts.next().ok_or(ParseError::BadStartLine)?;
        let status = parts.next().ok_or(ParseError::BadStartLine)?;
        let reason = parts.next().unwrap_or("");
        if !version.eq_ignore_ascii_case("SIP/2.0") {
            return Err(ParseError::BadVersion);
        }
        if status.len() != 3 || !status.bytes().all(|b| b.is_ascii_digit()) {
            return Err(ParseError::BadStartLine);
        }
        let status: u16 = status.parse().map_err(|_| ParseError::BadStartLine)?;
        if !(100..700).contains(&status) {
            return Err(ParseError::BadStartLine);
        }
        return Ok(Message::Response(Response {
            status,
            reason: reason.to_owned(),
            headers,
            body,
        }));
    }

    // Section 7.1: "Method SP Request-URI SP SIP-Version", single spaces.
    let mut parts = start.split(' ');
    let (Some(method), Some(uri), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(ParseError::BadStartLine);
    };
    let method = Method::parse(method).ok_or(ParseError::BadStartLine)?;
    if uri.is_empty()
        || uri
            .bytes()
            .any(|b| b <= b' ' || b == 0x7F || b"<>\"".contains(&b))
    {
        return Err(ParseError::BadStartLine);
    }
    if !version.eq_ignore_ascii_case("SIP/2.0") {
        return Err(
            if version
                .get(..4)
                .is_some_and(|p| p.eq_ignore_ascii_case("SIP/"))
            {
                ParseError::BadVersion
            } else {
                ParseError::BadStartLine
            },
        );
    }
    Ok(Message::Request(Request {
        method,
        uri: uri.to_owned(),
        headers,
        body,
    }))
}
