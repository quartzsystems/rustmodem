// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Digest authentication, client side (RFC 7616, RFC 2617; RFC 3261
//! section 22).
//!
//! MD5 and SHA-256, `qop=auth` with a nonce count and a client nonce, and
//! the older form with no `qop` (RFC 2617 section 3.2.2.1). The `-sess`
//! algorithms and `auth-int` are not offered by anything rustmodem
//! registers to and are not answered.
//!
//! The password is used to compute the response and nowhere else: it is
//! never put in a message, and [`Credentials`] does not print it.

use std::fmt;

use md5::{Digest as _, Md5};
use sha2::Sha256;

use crate::headers::Auth;
use crate::message::Response;
use crate::text::quote;

/// A hash algorithm of a challenge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Algorithm {
    /// The default when a challenge names none (RFC 2617 section 3.2.1).
    Md5,
    Sha256,
}

impl Algorithm {
    /// The algorithm a challenge names. `None` for one we do not have.
    pub fn parse(name: Option<&str>) -> Option<Algorithm> {
        match name {
            None => Some(Algorithm::Md5),
            Some(name) if name.eq_ignore_ascii_case("MD5") => Some(Algorithm::Md5),
            Some(name) if name.eq_ignore_ascii_case("SHA-256") => Some(Algorithm::Sha256),
            Some(_) => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Algorithm::Md5 => "MD5",
            Algorithm::Sha256 => "SHA-256",
        }
    }

    /// The hash of `data` in lower-case hexadecimal.
    pub fn hash(self, data: &[u8]) -> String {
        match self {
            Algorithm::Md5 => hex(&Md5::digest(data)),
            Algorithm::Sha256 => hex(&Sha256::digest(data)),
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(2 * bytes.len());
    for byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0F)]));
    }
    out
}

/// A user name and a password.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("username", &self.username)
            .field("password", &"(not shown)")
            .finish()
    }
}

/// The parts of a request a response is computed over.
#[derive(Clone, Copy, Debug)]
pub struct Subject<'a> {
    /// The method of the request, `REGISTER` or `INVITE`.
    pub method: &'a str,
    /// The Request-URI, as it is on the request line.
    pub uri: &'a str,
}

/// The `response` value (RFC 7616 section 3.4.1):
///
/// ```text
/// A1       = username ":" realm ":" password
/// A2       = method ":" request-uri
/// response = H( H(A1) ":" nonce ":" nc ":" cnonce ":" qop ":" H(A2) )
/// ```
///
/// and with no `qop`, `H( H(A1) ":" nonce ":" H(A2) )` (RFC 2617 section
/// 3.2.2.1). `qop` is the nonce count and the client nonce.
pub fn response(
    algorithm: Algorithm,
    credentials: &Credentials,
    realm: &str,
    nonce: &str,
    subject: Subject<'_>,
    qop: Option<(u32, &str)>,
) -> String {
    let a1 = format!(
        "{}:{}:{}",
        credentials.username, realm, credentials.password
    );
    let a2 = format!("{}:{}", subject.method, subject.uri);
    let ha1 = algorithm.hash(a1.as_bytes());
    let ha2 = algorithm.hash(a2.as_bytes());
    let data = match qop {
        Some((nc, cnonce)) => format!("{ha1}:{nonce}:{nc:08x}:{cnonce}:auth:{ha2}"),
        None => format!("{ha1}:{nonce}:{ha2}"),
    };
    algorithm.hash(data.as_bytes())
}

/// Why a challenge was not answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DigestError {
    /// The response has no Digest challenge we can answer: another scheme,
    /// an algorithm we do not have, or a `qop` without `auth`.
    NoChallenge,
    /// We answered and were challenged again with nothing to say the nonce
    /// was stale: the user name or the password is wrong.
    Rejected,
}

impl fmt::Display for DigestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            DigestError::NoChallenge => "the server asked for authentication we cannot give",
            DigestError::Rejected => "the server refused the user name or the password",
        })
    }
}

impl std::error::Error for DigestError {}

/// A challenge we hold and answer.
#[derive(Clone, Debug)]
struct Held {
    /// From a 407: answered in Proxy-Authorization.
    proxy: bool,
    algorithm: Algorithm,
    /// Whether the challenge named its algorithm; the answer names it then.
    algorithm_named: bool,
    realm: String,
    nonce: String,
    opaque: Option<String>,
    /// Whether the challenge offers `qop=auth`.
    qop: bool,
    /// How many requests have used the nonce (RFC 7616 section 3.4).
    count: u32,
}

/// The authentication of one user to one server, across the requests of a
/// registration or of a call.
///
/// A challenge gets one answer. A second challenge is answered only if it
/// says `stale=true` and brings a new nonce; any other is the server
/// refusing the credentials.
#[derive(Clone, Debug)]
pub struct DigestSession {
    credentials: Credentials,
    held: Option<Held>,
    /// How many challenges in a row have been answered.
    answered: u8,
}

impl DigestSession {
    pub fn new(credentials: Credentials) -> DigestSession {
        DigestSession {
            credentials,
            held: None,
            answered: 0,
        }
    }

    /// A 401 or a 407 came back. `Ok` means the request is to be sent
    /// again with [`DigestSession::authorization`].
    pub fn challenged(&mut self, response: &Response) -> Result<(), DigestError> {
        let proxy = response.status == 407;
        let challenges = if proxy {
            response.headers.proxy_authenticate()
        } else {
            response.headers.www_authenticate()
        };
        // RFC 7616 section 3.7: of the challenges we can answer, the
        // strongest.
        let Some((algorithm, challenge)) = challenges
            .iter()
            .filter(|challenge| challenge.is_digest())
            .filter(|challenge| {
                challenge.get("realm").is_some() && challenge.get("nonce").is_some()
            })
            .filter(|challenge| challenge.get("qop").is_none_or(offers_auth))
            .filter_map(|challenge| {
                Algorithm::parse(challenge.get("algorithm")).map(|algorithm| (algorithm, challenge))
            })
            .max_by_key(|(algorithm, _)| *algorithm)
        else {
            self.forget();
            return Err(DigestError::NoChallenge);
        };
        let nonce = challenge.get("nonce").unwrap_or_default();
        let stale = challenge
            .get("stale")
            .is_some_and(|stale| stale.eq_ignore_ascii_case("true"));
        let fresh = self.held.as_ref().is_none_or(|held| held.nonce != nonce);
        let retry = match self.answered {
            0 => true,
            1 => stale && fresh,
            _ => false,
        };
        if !retry {
            self.forget();
            return Err(DigestError::Rejected);
        }
        self.answered += 1;
        self.held = Some(Held {
            proxy,
            algorithm,
            algorithm_named: challenge.get("algorithm").is_some(),
            realm: challenge.get("realm").unwrap_or_default().to_owned(),
            nonce: nonce.to_owned(),
            opaque: challenge.get("opaque").map(str::to_owned),
            qop: challenge.get("qop").is_some(),
            count: 0,
        });
        Ok(())
    }

    /// The request was answered with something other than a challenge: the
    /// next challenge is a first one again. The nonce is kept, so the next
    /// request can carry credentials without being asked.
    pub fn accepted(&mut self) {
        self.answered = 0;
    }

    /// Forgets the challenge and starts over.
    pub fn forget(&mut self) {
        self.held = None;
        self.answered = 0;
    }

    /// The header to put on the next request: its name and its value.
    /// `None` when no challenge is held. `cnonce` is a fresh random string
    /// from the caller.
    pub fn authorization(
        &mut self,
        subject: Subject<'_>,
        cnonce: &str,
    ) -> Option<(&'static str, String)> {
        let held = self.held.as_mut()?;
        held.count += 1;
        let qop = held.qop.then_some((held.count, cnonce));
        let response = response(
            held.algorithm,
            &self.credentials,
            &held.realm,
            &held.nonce,
            subject,
            qop,
        );
        let mut value = format!(
            "Digest username={}, realm={}, nonce={}, uri={}, response=\"{response}\"",
            quote(&self.credentials.username),
            quote(&held.realm),
            quote(&held.nonce),
            quote(subject.uri),
        );
        if held.algorithm_named {
            value.push_str(&format!(", algorithm={}", held.algorithm.name()));
        }
        if let Some((count, cnonce)) = qop {
            value.push_str(&format!(
                ", cnonce={}, qop=auth, nc={count:08x}",
                quote(cnonce)
            ));
        }
        if let Some(opaque) = &held.opaque {
            value.push_str(&format!(", opaque={}", quote(opaque)));
        }
        let name = if held.proxy {
            "Proxy-Authorization"
        } else {
            "Authorization"
        };
        Some((name, value))
    }

    /// The user name the session answers with.
    pub fn username(&self) -> &str {
        &self.credentials.username
    }
}

/// Whether a `qop` value lists `auth`.
fn offers_auth(qop: &str) -> bool {
    qop.split(',')
        .any(|option| option.trim().eq_ignore_ascii_case("auth"))
}

/// A challenge with these parameters, for a server that asks: a test
/// peer. The header value of WWW-Authenticate or Proxy-Authenticate.
pub fn challenge(realm: &str, nonce: &str, algorithm: Algorithm, qop: bool, stale: bool) -> String {
    let mut value = format!(
        "Digest realm={}, nonce={}, algorithm={}",
        quote(realm),
        quote(nonce),
        algorithm.name()
    );
    if qop {
        value.push_str(", qop=\"auth\"");
    }
    if stale {
        value.push_str(", stale=true");
    }
    value
}

/// Whether credentials answer a challenge of ours, for a server that
/// checks: a test peer. The credentials must be for `nonce`.
pub fn verify(credentials: &Credentials, given: &Auth, nonce: &str, method: &str) -> bool {
    let (Some(username), Some(realm), Some(their_nonce), Some(uri), Some(theirs)) = (
        given.get("username"),
        given.get("realm"),
        given.get("nonce"),
        given.get("uri"),
        given.get("response"),
    ) else {
        return false;
    };
    let Some(algorithm) = Algorithm::parse(given.get("algorithm")) else {
        return false;
    };
    if !given.is_digest() || username != credentials.username || their_nonce != nonce {
        return false;
    }
    let qop = match (given.get("qop"), given.get("nc"), given.get("cnonce")) {
        (Some(qop), Some(nc), Some(cnonce)) if qop.eq_ignore_ascii_case("auth") => {
            let Ok(nc) = u32::from_str_radix(nc, 16) else {
                return false;
            };
            Some((nc, cnonce))
        }
        (None, _, _) => None,
        _ => return false,
    };
    let subject = Subject { method, uri };
    response(algorithm, credentials, realm, nonce, subject, qop) == theirs
}
