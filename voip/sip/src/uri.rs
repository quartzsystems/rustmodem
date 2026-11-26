// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The SIP URI (RFC 3261 section 19.1):
//! `sip:user:password@host:port;uri-parameters?headers`.
//!
//! Only `sip:` is taken. `sips:` needs TLS and `tel:` needs a gateway
//! mapping; both are refused, and so is every other scheme.

use std::fmt;

use crate::text::Params;

/// A `sip:` URI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SipUri {
    /// The user part as written, escapes and all.
    pub user: Option<String>,
    /// The password, which nobody should send (RFC 3261 section 19.1.1).
    pub password: Option<String>,
    /// A host name, an IPv4 address, or an IPv6 address in brackets.
    pub host: String,
    pub port: Option<u16>,
    pub params: Params,
    /// The headers after `?`, as written.
    pub headers: Vec<(String, String)>,
}

/// Why text is not a URI we take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UriError {
    /// White space, a control character, a quote or an angle bracket.
    BadCharacter,
    /// There is no scheme.
    NoScheme,
    /// The scheme is not `sip`.
    UnsupportedScheme,
    /// The user part is empty or has a character that must be escaped.
    BadUser,
    /// The host is empty or not a host.
    BadHost,
    /// The port is not a number from 0 to 65535.
    BadPort,
    /// A parameter name is not a token.
    BadParams,
    /// A header after `?` has no name.
    BadHeaders,
}

impl fmt::Display for UriError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            UriError::BadCharacter => "the URI has a character that must be escaped",
            UriError::NoScheme => "the URI has no scheme",
            UriError::UnsupportedScheme => "the URI scheme is not sip",
            UriError::BadUser => "the user part of the URI is not valid",
            UriError::BadHost => "the host of the URI is not valid",
            UriError::BadPort => "the port of the URI is not valid",
            UriError::BadParams => "the parameters of the URI are not valid",
            UriError::BadHeaders => "the headers of the URI are not valid",
        })
    }
}

impl std::error::Error for UriError {}

impl SipUri {
    /// `sip:host`.
    pub fn host(host: &str) -> SipUri {
        SipUri {
            user: None,
            password: None,
            host: host.to_owned(),
            port: None,
            params: Params::new(),
            headers: Vec::new(),
        }
    }

    /// `sip:user@host:port`.
    pub fn new(user: Option<&str>, host: &str, port: Option<u16>) -> SipUri {
        SipUri {
            user: user.map(str::to_owned),
            port,
            ..SipUri::host(host)
        }
    }

    pub fn parse(text: &str) -> Result<SipUri, UriError> {
        // None of these can be in a URI without being escaped.
        if text
            .bytes()
            .any(|b| b <= b' ' || b == 0x7F || b"<>\"".contains(&b))
        {
            return Err(UriError::BadCharacter);
        }
        let (scheme, rest) = text.split_once(':').ok_or(UriError::NoScheme)?;
        if !scheme.eq_ignore_ascii_case("sip") {
            return Err(UriError::UnsupportedScheme);
        }

        // The user part cannot hold an "@" that is not escaped, and nothing
        // after it can hold one at all.
        let (userinfo, rest) = match rest.split_once('@') {
            Some((userinfo, rest)) => (Some(userinfo), rest),
            None => (None, rest),
        };
        let (user, password) = match userinfo {
            None => (None, None),
            Some(userinfo) => {
                let (user, password) = match userinfo.split_once(':') {
                    Some((user, password)) => (user, Some(password)),
                    None => (userinfo, None),
                };
                if user.is_empty() || !user.bytes().all(user_char) {
                    return Err(UriError::BadUser);
                }
                if password.is_some_and(|p| !p.bytes().all(user_char)) {
                    return Err(UriError::BadUser);
                }
                (Some(user.to_owned()), password.map(str::to_owned))
            }
        };

        let (rest, headers) = match rest.split_once('?') {
            Some((rest, headers)) => (rest, Some(headers)),
            None => (rest, None),
        };
        let (hostport, params) = match rest.split_once(';') {
            Some((hostport, params)) => (hostport, Some(params)),
            None => (rest, None),
        };
        let (host, port) = split_host_port(hostport)?;

        let params = match params {
            Some(params) => Params::parse(params).ok_or(UriError::BadParams)?,
            None => Params::new(),
        };
        let mut parsed_headers = Vec::new();
        for header in headers.into_iter().flat_map(|h| h.split('&')) {
            let (name, value) = header.split_once('=').unwrap_or((header, ""));
            if name.is_empty() || header.bytes().any(|b| b.is_ascii_whitespace()) {
                return Err(UriError::BadHeaders);
            }
            parsed_headers.push((name.to_owned(), value.to_owned()));
        }

        Ok(SipUri {
            user,
            password,
            host: host.to_owned(),
            port,
            params,
            headers: parsed_headers,
        })
    }

    /// The user part with its `%hh` escapes undone, for reading a called
    /// number. An escape that is not valid is kept as written.
    pub fn user_decoded(&self) -> Option<String> {
        let user = self.user.as_deref()?;
        let bytes = user.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut at = 0;
        while at < bytes.len() {
            let escaped = (bytes[at] == b'%')
                .then(|| user.get(at + 1..at + 3))
                .flatten()
                .and_then(|hex| u8::from_str_radix(hex, 16).ok());
            match escaped {
                Some(byte) => {
                    out.push(byte);
                    at += 3;
                }
                None => {
                    out.push(bytes[at]);
                    at += 1;
                }
            }
        }
        Some(String::from_utf8_lossy(&out).into_owned())
    }

    /// The port to send to: the one given, or 5060 (RFC 3261 section
    /// 19.1.2).
    pub fn port_or_default(&self) -> u16 {
        self.port.unwrap_or(5060)
    }
}

impl fmt::Display for SipUri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("sip:")?;
        if let Some(user) = &self.user {
            f.write_str(user)?;
            if let Some(password) = &self.password {
                write!(f, ":{password}")?;
            }
            f.write_str("@")?;
        }
        f.write_str(&self.host)?;
        if let Some(port) = self.port {
            write!(f, ":{port}")?;
        }
        write!(f, "{}", self.params)?;
        for (n, (name, value)) in self.headers.iter().enumerate() {
            write!(f, "{}{name}={value}", if n == 0 { '?' } else { '&' })?;
        }
        Ok(())
    }
}

/// The characters of a user part or a password: unreserved, escaped (the
/// `%` and its hex digits), and the reserved ones RFC 3261 section 25.1
/// lets through.
fn user_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"-_.!~*'()%&=+$,;?/".contains(&b)
}

/// Splits `host`, `host:port` or `[v6]:port`.
pub(crate) fn split_host_port(text: &str) -> Result<(&str, Option<u16>), UriError> {
    let (host, port) = if text.starts_with('[') {
        let end = text.find(']').ok_or(UriError::BadHost)?;
        let (host, after) = text.split_at(end + 1);
        let inner = &host[1..end];
        if inner.is_empty()
            || !inner
                .bytes()
                .all(|b| b.is_ascii_hexdigit() || b":.".contains(&b))
        {
            return Err(UriError::BadHost);
        }
        match after.strip_prefix(':') {
            Some(port) => (host, Some(port)),
            None if after.is_empty() => (host, None),
            None => return Err(UriError::BadHost),
        }
    } else {
        let (host, port) = match text.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (text, None),
        };
        if host.is_empty()
            || !host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._".contains(&b))
        {
            return Err(UriError::BadHost);
        }
        (host, port)
    };
    let port = match port {
        None => None,
        Some(port) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => {
            Some(port.parse().map_err(|_| UriError::BadPort)?)
        }
        Some(_) => return Err(UriError::BadPort),
    };
    Ok((host, port))
}
