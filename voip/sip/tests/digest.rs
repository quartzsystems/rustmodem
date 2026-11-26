// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Digest authentication against the worked examples of the RFCs, and the
//! rule for when a challenge is answered.

use sip::digest::{self, Algorithm, Credentials, DigestError, DigestSession, Subject};
use sip::{Auth, Response};

fn mufasa(password: &str) -> Credentials {
    Credentials {
        username: "Mufasa".to_owned(),
        password: password.to_owned(),
    }
}

const GET: Subject<'static> = Subject {
    method: "GET",
    uri: "/dir/index.html",
};

/// RFC 2617 section 3.5.
#[test]
fn the_example_of_rfc_2617() {
    let response = digest::response(
        Algorithm::Md5,
        &mufasa("Circle Of Life"),
        "testrealm@host.com",
        "dcd98b7102dd2f0e8b11d0f600bfb0c093",
        GET,
        Some((1, "0a4f113b")),
    );
    assert_eq!(response, "6629fae49393a05397450978507c4ef1");
}

/// RFC 7616 section 3.9.1, with SHA-256 and with MD5.
#[test]
fn the_examples_of_rfc_7616() {
    let nonce = "7ypf/xlj9XXwfDPEoM4URrv/xwf94BcCAzFZH4GiTo0v";
    let cnonce = "f2/wE4q74E6zIJEtWaHKaf5wv/H5QzzpXusqGemxURZJ";
    let credentials = mufasa("Circle of Life");
    let realm = "http-auth@example.org";
    assert_eq!(
        digest::response(
            Algorithm::Sha256,
            &credentials,
            realm,
            nonce,
            GET,
            Some((1, cnonce))
        ),
        "753927fa0e85d155564e2e272a28d1802ca10daf4496794697cf8db5856cb6c1"
    );
    assert_eq!(
        digest::response(
            Algorithm::Md5,
            &credentials,
            realm,
            nonce,
            GET,
            Some((1, cnonce))
        ),
        "8ca523f5e9506fed4657c9700eebdbec"
    );
}

/// RFC 2617 section 3.2.2.1: with no qop the response is over the nonce
/// alone, as RFC 2069 had it. The value is the one of RFC 2069 section
/// 2.4 as corrected by its erratum.
#[test]
fn the_form_without_qop() {
    let response = digest::response(
        Algorithm::Md5,
        &mufasa("CircleOfLife"),
        "testrealm@host.com",
        "dcd98b7102dd2f0e8b11d0f600bfb0c093",
        GET,
        None,
    );
    assert_eq!(response, "1949323746fe6a43ef61f9606e7febea");
}

#[test]
fn the_hashes() {
    assert_eq!(Algorithm::Md5.hash(b""), "d41d8cd98f00b204e9800998ecf8427e");
    assert_eq!(
        Algorithm::Sha256.hash(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(Algorithm::parse(None), Some(Algorithm::Md5));
    assert_eq!(Algorithm::parse(Some("md5")), Some(Algorithm::Md5));
    assert_eq!(Algorithm::parse(Some("sha-256")), Some(Algorithm::Sha256));
    assert_eq!(Algorithm::parse(Some("MD5-sess")), None);
    assert_eq!(Algorithm::parse(Some("SHA-512-256")), None);
}

fn challenge(status: u16, lines: &[&str]) -> Response {
    let mut response = Response::new(status);
    let name = if status == 407 {
        "Proxy-Authenticate"
    } else {
        "WWW-Authenticate"
    };
    for line in lines {
        response.headers.push(name, *line);
    }
    response
}

const REGISTER: Subject<'static> = Subject {
    method: "REGISTER",
    uri: "sip:pbx.example.lan",
};

fn session() -> DigestSession {
    DigestSession::new(Credentials {
        username: "7002".to_owned(),
        password: "secret".to_owned(),
    })
}

fn credentials_of(value: &str) -> Auth {
    Auth::parse(value).expect("the header value parses")
}

#[test]
fn a_challenge_is_answered_with_what_it_asks_for() {
    let mut s = session();
    assert_eq!(s.authorization(REGISTER, "c1"), None);
    s.challenged(&challenge(
        401,
        &["Digest realm=\"pbx\", nonce=\"n1\", algorithm=MD5, qop=\"auth\", opaque=\"op\""],
    ))
    .unwrap();
    let (name, value) = s.authorization(REGISTER, "c1").unwrap();
    assert_eq!(name, "Authorization");
    let expected = digest::response(
        Algorithm::Md5,
        &Credentials {
            username: "7002".to_owned(),
            password: "secret".to_owned(),
        },
        "pbx",
        "n1",
        REGISTER,
        Some((1, "c1")),
    );
    assert_eq!(
        value,
        format!(
            "Digest username=\"7002\", realm=\"pbx\", nonce=\"n1\", uri=\"sip:pbx.example.lan\", \
             response=\"{expected}\", algorithm=MD5, cnonce=\"c1\", qop=auth, nc=00000001, opaque=\"op\""
        )
    );
    // The password is not in it.
    assert!(!value.contains("secret"));
    assert_eq!(s.username(), "7002");

    // The same nonce again: the count goes up.
    let (_, second) = s.authorization(REGISTER, "c2").unwrap();
    let second = credentials_of(&second);
    assert_eq!(second.get("nc"), Some("00000002"));
    assert_eq!(second.get("cnonce"), Some("c2"));
}

#[test]
fn a_challenge_without_qop_or_algorithm_is_answered_the_old_way() {
    let mut s = session();
    s.challenged(&challenge(407, &["Digest realm=\"pbx\", nonce=\"n1\""]))
        .unwrap();
    let (name, value) = s.authorization(REGISTER, "unused").unwrap();
    assert_eq!(name, "Proxy-Authorization");
    let given = credentials_of(&value);
    assert_eq!(given.get("qop"), None);
    assert_eq!(given.get("nc"), None);
    assert_eq!(given.get("cnonce"), None);
    assert_eq!(given.get("algorithm"), None);
    assert_eq!(given.get("response").unwrap().len(), 32);
}

#[test]
fn the_strongest_challenge_we_can_answer_is_taken() {
    let mut s = session();
    s.challenged(&challenge(
        401,
        &[
            "Digest realm=\"pbx\", nonce=\"weak\", algorithm=MD5, qop=\"auth\"",
            "Digest realm=\"pbx\", nonce=\"strong\", algorithm=SHA-256, qop=\"auth, auth-int\"",
            "Digest realm=\"pbx\", nonce=\"unknown\", algorithm=SHA-512-256, qop=\"auth\"",
            "Basic realm=\"pbx\"",
        ],
    ))
    .unwrap();
    let (_, value) = s.authorization(REGISTER, "c").unwrap();
    let given = credentials_of(&value);
    assert_eq!(given.get("nonce"), Some("strong"));
    assert_eq!(given.get("algorithm"), Some("SHA-256"));
    assert_eq!(given.get("response").unwrap().len(), 64);
}

#[test]
fn challenges_we_cannot_answer() {
    for lines in [
        &[][..],
        &["Basic realm=\"pbx\""][..],
        &["Digest realm=\"pbx\", nonce=\"n\", algorithm=MD5-sess"][..],
        &["Digest realm=\"pbx\", nonce=\"n\", qop=\"auth-int\""][..],
        &["Digest realm=\"pbx\""][..],
        &["Digest nonce=\"n\""][..],
        &["not a challenge at all"][..],
    ] {
        let mut s = session();
        assert_eq!(
            s.challenged(&challenge(401, lines)),
            Err(DigestError::NoChallenge),
            "{lines:?}"
        );
        assert_eq!(s.authorization(REGISTER, "c"), None);
    }
    // A 407 is answered from Proxy-Authenticate only.
    let mut s = session();
    let mut wrong = Response::new(407);
    wrong
        .headers
        .push("WWW-Authenticate", "Digest realm=\"pbx\", nonce=\"n\"");
    assert_eq!(s.challenged(&wrong), Err(DigestError::NoChallenge));
}

#[test]
fn a_second_challenge_is_answered_only_when_the_nonce_was_stale() {
    let first = challenge(401, &["Digest realm=\"pbx\", nonce=\"n1\", qop=\"auth\""]);

    // The same again: the credentials are wrong.
    let mut s = session();
    s.challenged(&first).unwrap();
    assert_eq!(s.challenged(&first), Err(DigestError::Rejected));
    assert_eq!(s.authorization(REGISTER, "c"), None);

    // A new nonce that is not said to be stale: wrong as well.
    let mut s = session();
    s.challenged(&first).unwrap();
    let other = challenge(401, &["Digest realm=\"pbx\", nonce=\"n2\", qop=\"auth\""]);
    assert_eq!(s.challenged(&other), Err(DigestError::Rejected));

    // Stale, but the same nonce: nothing new to answer with.
    let mut s = session();
    s.challenged(&first).unwrap();
    let same = challenge(
        401,
        &["Digest realm=\"pbx\", nonce=\"n1\", qop=\"auth\", stale=true"],
    );
    assert_eq!(s.challenged(&same), Err(DigestError::Rejected));

    // Stale with a new nonce: answered, once.
    let mut s = session();
    s.challenged(&first).unwrap();
    let stale = challenge(
        401,
        &["Digest realm=\"pbx\", nonce=\"n2\", qop=\"auth\", stale=TRUE"],
    );
    s.challenged(&stale).unwrap();
    let (_, value) = s.authorization(REGISTER, "c").unwrap();
    let given = credentials_of(&value);
    assert_eq!(given.get("nonce"), Some("n2"));
    assert_eq!(given.get("nc"), Some("00000001"));
    let again = challenge(
        401,
        &["Digest realm=\"pbx\", nonce=\"n3\", qop=\"auth\", stale=true"],
    );
    assert_eq!(s.challenged(&again), Err(DigestError::Rejected));

    // After a failure the next challenge is a first one again.
    s.challenged(&first).unwrap();
}

#[test]
fn after_a_request_is_accepted_the_next_challenge_is_a_first_one() {
    let mut s = session();
    s.challenged(&challenge(
        401,
        &["Digest realm=\"pbx\", nonce=\"n1\", qop=\"auth\""],
    ))
    .unwrap();
    s.authorization(REGISTER, "c").unwrap();
    s.accepted();
    // The nonce is kept and used without being asked.
    let (_, value) = s.authorization(REGISTER, "c").unwrap();
    assert_eq!(credentials_of(&value).get("nc"), Some("00000002"));
    // The registrar's nonce ran out and it does not say stale.
    s.challenged(&challenge(
        401,
        &["Digest realm=\"pbx\", nonce=\"n2\", qop=\"auth\""],
    ))
    .unwrap();
    let (_, value) = s.authorization(REGISTER, "c").unwrap();
    let given = credentials_of(&value);
    assert_eq!(
        (given.get("nonce"), given.get("nc")),
        (Some("n2"), Some("00000001"))
    );
    s.forget();
    assert_eq!(s.authorization(REGISTER, "c"), None);
}

#[test]
fn values_with_quotes_are_escaped() {
    let mut s = DigestSession::new(Credentials {
        username: "a\"b".to_owned(),
        password: "p".to_owned(),
    });
    s.challenged(&challenge(
        401,
        &["Digest realm=\"r \\\"x\\\"\", nonce=\"n\\\\1\""],
    ))
    .unwrap();
    let (_, value) = s.authorization(REGISTER, "c").unwrap();
    let given = credentials_of(&value);
    assert_eq!(given.get("username"), Some("a\"b"));
    assert_eq!(given.get("realm"), Some("r \"x\""));
    assert_eq!(given.get("nonce"), Some("n\\1"));
}

#[test]
fn the_password_is_not_printed() {
    let credentials = Credentials {
        username: "7002".to_owned(),
        password: "hunter2".to_owned(),
    };
    let printed = format!(
        "{credentials:?} {:?}",
        DigestSession::new(credentials.clone())
    );
    assert!(printed.contains("7002"));
    assert!(!printed.contains("hunter2"));
}

/// The two halves a test peer uses: a challenge of ours, and the check of
/// the credentials that answer it.
#[test]
fn a_challenge_of_ours_is_answered_and_the_answer_checks() {
    let credentials = Credentials {
        username: "7002".to_owned(),
        password: "secret".to_owned(),
    };
    for algorithm in [Algorithm::Md5, Algorithm::Sha256] {
        for qop in [true, false] {
            let line = digest::challenge("pbx", "nonce-1", algorithm, qop, false);
            let mut s = DigestSession::new(credentials.clone());
            s.challenged(&challenge(401, &[&line])).unwrap();
            let (_, value) = s.authorization(REGISTER, "cn").unwrap();
            let given = credentials_of(&value);
            assert!(digest::verify(&credentials, &given, "nonce-1", "REGISTER"));
            // Another method, another nonce, another password: refused.
            assert!(!digest::verify(&credentials, &given, "nonce-1", "INVITE"));
            assert!(!digest::verify(&credentials, &given, "nonce-2", "REGISTER"));
            let wrong = Credentials {
                password: "Secret".to_owned(),
                ..credentials.clone()
            };
            assert!(!digest::verify(&wrong, &given, "nonce-1", "REGISTER"));
        }
    }
    assert!(digest::challenge("pbx", "n", Algorithm::Md5, true, true).ends_with(", stale=true"));
    assert!(!digest::verify(
        &credentials,
        &credentials_of("Basic realm=\"pbx\""),
        "n",
        "REGISTER"
    ));
}
