// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Messages and typed header access, case by case.

use sip::{
    Auth, CSeq, Contacts, Headers, Invalid, MAX_DATAGRAM, MAX_HEADERS, MAX_LINE, Message, Method,
    NameAddr, ParseError, Request, Response, SipUri, Via, reason_phrase,
};

const INVITE: &str = "INVITE sip:5551000@10.10.20.5 SIP/2.0\r\n\
Via: SIP/2.0/UDP 10.10.20.11:5060;branch=z9hG4bK-524287-1;rport\r\n\
Max-Forwards: 70\r\n\
From: \"Front desk\" <sip:201@10.10.20.11>;tag=8f2a\r\n\
To: <sip:5551000@10.10.20.5>\r\n\
Call-ID: 3b1c9e@10.10.20.11\r\n\
CSeq: 101 INVITE\r\n\
Contact: <sip:201@10.10.20.11:5060>\r\n\
User-Agent: Example ATA 1.4\r\n\
Allow: INVITE, ACK, BYE, CANCEL, OPTIONS\r\n\
Supported: replaces, timer\r\n\
Content-Type: application/sdp\r\n\
Content-Length: 4\r\n\
\r\n\
v=0\n";

fn request(text: &str) -> Request {
    match Message::parse(text.as_bytes()) {
        Ok(Message::Request(request)) => request,
        other => panic!("not a request: {other:?}"),
    }
}

fn response(text: &str) -> Response {
    match Message::parse(text.as_bytes()) {
        Ok(Message::Response(response)) => response,
        other => panic!("not a response: {other:?}"),
    }
}

#[test]
fn a_request() {
    let r = request(INVITE);
    assert_eq!(r.method, Method::Invite);
    assert_eq!(r.uri, "sip:5551000@10.10.20.5");
    assert_eq!(r.sip_uri().unwrap().user.as_deref(), Some("5551000"));
    assert_eq!(r.body, b"v=0\n");
    // Content-Length is the body's; it is not kept as a header.
    assert_eq!(r.headers.0.len(), 11);
    assert!(!r.headers.has("Content-Length"));

    let via = r.headers.top_via().unwrap();
    assert_eq!(
        (via.transport.as_str(), via.host.as_str(), via.port),
        ("UDP", "10.10.20.11", Some(5060))
    );
    assert_eq!(via.branch(), Some("z9hG4bK-524287-1"));
    assert!(via.params.has("rport"));
    assert_eq!(via.params.get("rport"), None);

    let from = r.headers.from().unwrap();
    assert_eq!(from.display.as_deref(), Some("Front desk"));
    assert_eq!(from.uri, "sip:201@10.10.20.11");
    assert_eq!(from.tag(), Some("8f2a"));
    let to = r.headers.to().unwrap();
    assert_eq!((to.display.as_deref(), to.tag()), (None, None));

    assert_eq!(r.headers.call_id(), Some("3b1c9e@10.10.20.11"));
    assert_eq!(
        r.headers.cseq(),
        Some(CSeq {
            sequence: 101,
            method: "INVITE".to_owned()
        })
    );
    assert_eq!(r.headers.max_forwards(), Some(70));
    assert_eq!(
        r.headers.contact().unwrap().sip_uri().unwrap().port,
        Some(5060)
    );
    assert_eq!(r.headers.user_agent(), Some("Example ATA 1.4"));
    assert_eq!(
        r.headers.allow(),
        ["INVITE", "ACK", "BYE", "CANCEL", "OPTIONS"]
    );
    assert_eq!(r.headers.supported(), ["replaces", "timer"]);
    assert!(r.headers.require().is_empty());
    assert_eq!(r.headers.content_type().as_deref(), Some("application/sdp"));
    assert_eq!(r.headers.expires(), None);

    let e = r.essentials().unwrap();
    assert_eq!(e.call_id, "3b1c9e@10.10.20.11");
    assert_eq!(e.cseq.sequence, 101);
    assert_eq!(e.via.branch(), Some("z9hG4bK-524287-1"));

    assert_eq!(r.to_bytes(), INVITE.as_bytes());
}

#[test]
fn a_response() {
    let text = "SIP/2.0 180 Ringing\r\n\
Via: SIP/2.0/UDP 10.10.20.11:5060;branch=z9hG4bK-1;received=10.10.20.11;rport=5060\r\n\
From: <sip:201@10.10.20.11>;tag=8f2a\r\n\
To: <sip:5551000@10.10.20.5>;tag=77\r\n\
Call-ID: 3b1c9e@10.10.20.11\r\n\
CSeq: 101 INVITE\r\n\
Content-Length: 0\r\n\
\r\n";
    let r = response(text);
    assert_eq!((r.status, r.reason.as_str()), (180, "Ringing"));
    assert!(r.is_provisional());
    assert!(!r.is_success());
    assert!(r.body.is_empty());
    let e = r.essentials().unwrap();
    assert_eq!(e.to.tag(), Some("77"));
    assert_eq!(e.via.params.get("rport"), Some("5060"));
    assert_eq!(r.to_bytes(), text.as_bytes());
}

#[test]
fn status_lines() {
    for (line, status, reason) in [
        ("SIP/2.0 200 OK", 200, "OK"),
        ("SIP/2.0 200 ", 200, ""),
        ("SIP/2.0 200", 200, ""),
        ("sip/2.0 486 Busy Here", 486, "Busy Here"),
        (
            "SIP/2.0 699 = \u{2206}\u{2202} *",
            699,
            "= \u{2206}\u{2202} *",
        ),
        ("SIP/2.0 100  two spaces", 100, " two spaces"),
    ] {
        let r = response(&format!("{line}\r\n\r\n"));
        assert_eq!((r.status, r.reason.as_str()), (status, reason), "{line}");
    }
    for (line, error) in [
        ("SIP/2.0", ParseError::BadStartLine),
        ("SIP/2.0 20 OK", ParseError::BadStartLine),
        ("SIP/2.0 2000 OK", ParseError::BadStartLine),
        ("SIP/2.0 099 Low", ParseError::BadStartLine),
        ("SIP/2.0 700 High", ParseError::BadStartLine),
        (
            "SIP/2.0 4294967301 better not break the receiver",
            ParseError::BadStartLine,
        ),
        ("SIP/2.0 2x0 OK", ParseError::BadStartLine),
        ("SIP/2.0  200 OK", ParseError::BadStartLine),
        ("SIP/3.0 200 OK", ParseError::BadVersion),
        ("SIP/2.00 200 OK", ParseError::BadVersion),
    ] {
        assert_eq!(
            Message::parse(format!("{line}\r\n\r\n").as_bytes()),
            Err(error),
            "{line}"
        );
    }
}

#[test]
fn request_lines() {
    for (line, method) in [
        ("INVITE sip:a@example.com SIP/2.0", Method::Invite),
        ("ACK sip:a@example.com SIP/2.0", Method::Ack),
        ("BYE sip:a@example.com SIP/2.0", Method::Bye),
        ("CANCEL sip:a@example.com SIP/2.0", Method::Cancel),
        ("OPTIONS sip:a@example.com sip/2.0", Method::Options),
        ("REGISTER sip:example.com SIP/2.0", Method::Register),
        (
            "NEWMETHOD sip:a@example.com SIP/2.0",
            Method::Other("NEWMETHOD".to_owned()),
        ),
        // Method names are case sensitive.
        (
            "invite sip:a@example.com SIP/2.0",
            Method::Other("invite".to_owned()),
        ),
        // A "%" in a method is a character of it, not an escape.
        (
            "RE%47IST%45R sip:example.com SIP/2.0",
            Method::Other("RE%47IST%45R".to_owned()),
        ),
        ("OPTIONS tel:+15551000 SIP/2.0", Method::Options),
    ] {
        assert_eq!(request(&format!("{line}\r\n\r\n")).method, method, "{line}");
    }
    for (line, error) in [
        ("INVITE sip:a@example.com", ParseError::BadStartLine),
        (
            "INVITE  sip:a@example.com  SIP/2.0",
            ParseError::BadStartLine,
        ),
        (
            "INVITE sip:a@example.com SIP/2.0 ",
            ParseError::BadStartLine,
        ),
        (
            "INVITE sip:a@example.com; lr SIP/2.0",
            ParseError::BadStartLine,
        ),
        (
            "INVITE <sip:a@example.com> SIP/2.0",
            ParseError::BadStartLine,
        ),
        (
            "INVITE sip:a@example.com\tSIP/2.0",
            ParseError::BadStartLine,
        ),
        (
            "IN(VITE sip:a@example.com SIP/2.0",
            ParseError::BadStartLine,
        ),
        ("INVITE sip:a@example.com SIP/7.0", ParseError::BadVersion),
        (
            "INVITE sip:a@example.com HTTP/1.1",
            ParseError::BadStartLine,
        ),
        ("just some words", ParseError::BadStartLine),
        ("oneword", ParseError::BadStartLine),
    ] {
        assert_eq!(
            Message::parse(format!("{line}\r\n\r\n").as_bytes()),
            Err(error),
            "{line}"
        );
    }
}

#[test]
fn compact_forms_and_case() {
    let r = request(
        "OPTIONS sip:a@example.com SIP/2.0\r\n\
v: SIP/2.0/UDP 192.0.2.1;branch=z9hG4bKa\r\n\
f: <sip:b@example.com>;tag=1\r\n\
t: <sip:a@example.com>\r\n\
i: abc\r\n\
cseq: 1 OPTIONS\r\n\
m: <sip:b@192.0.2.1>\r\n\
k: path\r\n\
C: text/plain; charset=utf-8\r\n\
MAX-FORWARDS: 5\r\n\
s: hello\r\n\
x-custom: Kept\r\n\
l: 2\r\n\
\r\n\
hi",
    );
    let names: Vec<&str> = r.headers.0.iter().map(|h| h.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Via",
            "From",
            "To",
            "Call-ID",
            "CSeq",
            "Contact",
            "Supported",
            "Content-Type",
            "Max-Forwards",
            "Subject",
            "x-custom"
        ]
    );
    assert_eq!(r.body, b"hi");
    assert_eq!(r.headers.content_type().as_deref(), Some("text/plain"));
    // Lookups ignore case and take either form.
    assert_eq!(r.headers.get("call-id"), Some("abc"));
    assert_eq!(r.headers.get("i"), Some("abc"));
    assert_eq!(r.headers.get("X-CUSTOM"), Some("Kept"));
    assert!(r.essentials().is_ok());
}

#[test]
fn folded_headers_read_as_one_line() {
    let r = request(
        "OPTIONS sip:a@example.com SIP/2.0\r\n\
Subject: one\r\n two\r\n\tthree\r\n\
Empty:\r\n   filled later\r\n\
\r\n",
    );
    assert_eq!(r.headers.get("Subject"), Some("one two three"));
    assert_eq!(r.headers.get("Empty"), Some("filled later"));
    assert_eq!(
        Message::parse(b"OPTIONS sip:a@example.com SIP/2.0\r\n folded: nothing\r\n\r\n"),
        Err(ParseError::BadHeader)
    );
}

#[test]
fn headers_given_more_than_once_and_as_lists() {
    let r = request(
        "INVITE sip:a@example.com SIP/2.0\r\n\
Via: SIP/2.0/UDP one.example.com;branch=z9hG4bK1, SIP/2.0/UDP two.example.com:5070;branch=z9hG4bK2\r\n\
Via: SIP/2.0/UDP [2001:db8::3]:5080;branch=z9hG4bK3\r\n\
Route: <sip:p1.example.com;lr>, <sip:p2.example.com;lr>\r\n\
Record-Route: <sip:p2.example.com;lr>\r\n\
Record-Route: <sip:p1.example.com;lr>\r\n\
Contact: \"A, B\" <sip:ab@example.com>;q=0.7, sip:c@example.com;expires=60\r\n\
Require: 100rel\r\n\
Require: precondition , foo\r\n\
\r\n",
    );
    let vias = r.headers.vias().unwrap();
    let hosts: Vec<_> = vias.iter().map(|v| (v.host.as_str(), v.port)).collect();
    assert_eq!(
        hosts,
        [
            ("one.example.com", None),
            ("two.example.com", Some(5070)),
            ("[2001:db8::3]", Some(5080))
        ]
    );
    assert_eq!(vias[0].port_or_default(), 5060);
    assert_eq!(r.headers.top_via().unwrap().branch(), Some("z9hG4bK1"));

    let routes = r.headers.routes().unwrap();
    assert_eq!(routes.len(), 2);
    assert!(routes[0].sip_uri().unwrap().params.has("lr"));
    let record: Vec<String> = r
        .headers
        .record_routes()
        .unwrap()
        .into_iter()
        .map(|a| a.uri)
        .collect();
    assert_eq!(record, ["sip:p2.example.com;lr", "sip:p1.example.com;lr"]);

    let Some(Contacts::List(contacts)) = r.headers.contacts() else {
        panic!("contacts");
    };
    assert_eq!(contacts.len(), 2);
    assert_eq!(contacts[0].display.as_deref(), Some("A, B"));
    assert_eq!(contacts[0].params.get("q"), Some("0.7"));
    // Without brackets the parameters are the header's, not the URI's.
    assert_eq!(contacts[1].uri, "sip:c@example.com");
    assert_eq!(contacts[1].params.get("expires"), Some("60"));
    assert_eq!(r.headers.require(), ["100rel", "precondition", "foo"]);

    let star = request("REGISTER sip:example.com SIP/2.0\r\nContact: *\r\nExpires: 0\r\n\r\n");
    assert_eq!(star.headers.contacts(), Some(Contacts::Star));
    assert_eq!(star.headers.contact(), None);
    assert_eq!(star.headers.expires(), Some(0));
}

#[test]
fn name_addr_forms() {
    let cases: &[(&str, Option<&str>, &str, Option<&str>)] = &[
        ("<sip:a@example.com>", None, "sip:a@example.com", None),
        ("sip:a@example.com", None, "sip:a@example.com", None),
        (
            "sip:a@example.com;tag=9",
            None,
            "sip:a@example.com",
            Some("9"),
        ),
        (
            "<sip:a@example.com;transport=udp>;tag=9",
            None,
            "sip:a@example.com;transport=udp",
            Some("9"),
        ),
        (
            "Bob <sip:a@example.com>",
            Some("Bob"),
            "sip:a@example.com",
            None,
        ),
        (
            "Bob   Smith <sip:a@example.com> ; tag = 9",
            Some("Bob Smith"),
            "sip:a@example.com",
            Some("9"),
        ),
        (
            "caller<sip:a@example.com>;tag=9",
            Some("caller"),
            "sip:a@example.com",
            Some("9"),
        ),
        (
            "\"\" <sip:a@example.com>",
            Some(""),
            "sip:a@example.com",
            None,
        ),
        (
            "\"J \\\"Q\\\" <x>; P\" <sip:a@example.com>;tag=9",
            Some("J \"Q\" <x>; P"),
            "sip:a@example.com",
            Some("9"),
        ),
        (
            "Anonymous <tel:+15551000>;tag=9",
            Some("Anonymous"),
            "tel:+15551000",
            Some("9"),
        ),
    ];
    for (text, display, uri, tag) in cases {
        let addr = NameAddr::parse(text).unwrap_or_else(|| panic!("{text}"));
        assert_eq!(addr.display.as_deref(), *display, "{text}");
        assert_eq!(addr.uri, *uri, "{text}");
        assert_eq!(addr.tag(), *tag, "{text}");
        // Written and read again it is the same address.
        assert_eq!(
            NameAddr::parse(&addr.to_string()),
            Some(addr.clone()),
            "{text}"
        );
    }
    for text in [
        "",
        "<>",
        "<sip:a@example.com",
        "\"Mr. J. User <sip:j.user@example.com>",
        "\"Joe\" sip:joe@example.org",
        "<sip:a@example.com> trailing",
        "<sip:a@example.com>;=bad",
        "<sip:a b@example.com>",
    ] {
        assert_eq!(NameAddr::parse(text), None, "{text:?}");
    }
    let addr = NameAddr::new(&SipUri::new(Some("7002"), "pbx.example.lan", None));
    assert_eq!(addr.to_string(), "<sip:7002@pbx.example.lan>");
}

#[test]
fn via_forms() {
    let via =
        Via::parse("SIP / 2.0 / UDP   host.example.com : 5070 ; branch = z9hG4bKx ;rport").unwrap();
    assert_eq!(
        via.to_string(),
        "SIP/2.0/UDP host.example.com:5070;branch=z9hG4bKx;rport"
    );
    assert_eq!(Via::parse(&via.to_string()), Some(via));
    for text in [
        "",
        "SIP/2.0/UDP",
        "SIP/2.0 host.example.com",
        "SIP/3.0/UDP host.example.com",
        "HTTP/2.0/UDP host.example.com",
        "SIP/2.0/U(DP host.example.com",
        "SIP/2.0/UDP host.example.com:port",
        "SIP/2.0/UDP ho$t",
        "SIP/2.0/UDP host.example.com;=x",
    ] {
        assert_eq!(Via::parse(text), None, "{text:?}");
    }
}

#[test]
fn cseq_forms() {
    assert_eq!(
        CSeq::parse("0009   INVITE").unwrap().to_string(),
        "9 INVITE"
    );
    assert_eq!(
        CSeq::parse("2147483647 BYE").unwrap().sequence,
        2_147_483_647
    );
    for text in [
        "",
        "INVITE",
        "1",
        "1 INVITE extra",
        "-1 INVITE",
        "2147483648 INVITE",
        "36893488147419103232 REGISTER",
        "1 IN VITE",
        "x INVITE",
    ] {
        assert_eq!(CSeq::parse(text), None, "{text:?}");
    }
}

#[test]
fn challenges_and_credentials() {
    let r = response(
        "SIP/2.0 401 Unauthorized\r\n\
WWW-Authenticate: Digest realm=\"pbx.example.lan\", nonce=\"a,b=\\\"c\\\"\", algorithm=SHA-256, qop=\"auth,auth-int\", stale=TRUE, opaque=\"\"\r\n\
WWW-Authenticate: Digest realm=\"pbx.example.lan\",nonce=\"n2\",algorithm=MD5\r\n\
Proxy-Authenticate: Basic realm=\"x\"\r\n\
Proxy-Authenticate: no-parameters\r\n\
\r\n",
    );
    let challenges = r.headers.www_authenticate();
    assert_eq!(challenges.len(), 2);
    let first = &challenges[0];
    assert!(first.is_digest());
    assert_eq!(first.get("realm"), Some("pbx.example.lan"));
    assert_eq!(first.get("nonce"), Some("a,b=\"c\""));
    assert_eq!(first.get("ALGORITHM"), Some("SHA-256"));
    assert_eq!(first.get("qop"), Some("auth,auth-int"));
    assert_eq!(first.get("stale"), Some("TRUE"));
    assert_eq!(first.get("opaque"), Some(""));
    assert_eq!(first.get("domain"), None);
    assert_eq!(challenges[1].get("nonce"), Some("n2"));
    // The line that is not a challenge is left out.
    let proxy = r.headers.proxy_authenticate();
    assert_eq!(proxy.len(), 1);
    assert!(!proxy[0].is_digest());

    let credentials = Auth::parse(
        "Digest username=\"7002\", realm=\"pbx\", nonce=\"n\", uri=\"sip:pbx\", response=\"0123\", nc=00000001",
    )
    .unwrap();
    assert_eq!(credentials.get("nc"), Some("00000001"));
    assert_eq!(credentials.get("uri"), Some("sip:pbx"));
    for text in [
        "Digest",
        "Digest realm",
        "Digest =x",
        "Digest realm=\"open",
        "Di gest realm=x",
    ] {
        assert_eq!(Auth::parse(text), None, "{text:?}");
    }

    let mut request = Request::new(Method::Register, &SipUri::host("pbx"));
    request
        .headers
        .push("Authorization", "Digest username=\"a\", realm=\"b\"");
    request
        .headers
        .push("Proxy-Authorization", "Digest username=\"c\", realm=\"d\"");
    assert_eq!(
        request.headers.authorization()[0].get("username"),
        Some("a")
    );
    assert_eq!(
        request.headers.proxy_authorization()[0].get("realm"),
        Some("d")
    );
}

#[test]
fn the_body_and_content_length() {
    let head = "OPTIONS sip:a@example.com SIP/2.0\r\n";
    let parse = |rest: &str| Message::parse(format!("{head}{rest}").as_bytes());
    let body = |rest: &str| parse(rest).map(|m| m.body().to_vec());

    // Without Content-Length the body runs to the end of the datagram.
    assert_eq!(body("\r\nabc"), Ok(b"abc".to_vec()));
    assert_eq!(body("\r\n"), Ok(Vec::new()));
    // Octets after the body are dropped (RFC 3261 section 18.3).
    assert_eq!(body("Content-Length: 2\r\n\r\nabcdef"), Ok(b"ab".to_vec()));
    assert_eq!(body("Content-Length: 0\r\n\r\nabcdef"), Ok(Vec::new()));
    assert_eq!(body("l: 003\r\n\r\nabc"), Ok(b"abc".to_vec()));
    // Given twice with the same value is one value.
    assert_eq!(
        body("Content-Length: 3\r\nl: 3\r\n\r\nabc"),
        Ok(b"abc".to_vec())
    );
    // The body is octets, not text.
    let mut binary = format!("{head}Content-Length: 4\r\n\r\n").into_bytes();
    binary.extend_from_slice(&[0, 0xFF, b'\r', b'\n']);
    assert_eq!(
        Message::parse(&binary).unwrap().body(),
        [0, 0xFF, b'\r', b'\n']
    );

    assert_eq!(
        body("Content-Length: 4\r\n\r\nabc"),
        Err(ParseError::Truncated)
    );
    assert_eq!(
        body("Content-Length: 9999999999\r\n\r\nabc"),
        Err(ParseError::Truncated)
    );
    for bad in [
        "Content-Length: -999\r\n\r\nabc",
        "Content-Length: \r\n\r\nabc",
        "Content-Length: 3 3\r\n\r\nabc",
        "Content-Length: three\r\n\r\nabc",
        "Content-Length: 99999999999999999999999999\r\n\r\nabc",
        "Content-Length: 3\r\nContent-Length: 2\r\n\r\nabc",
    ] {
        assert_eq!(body(bad), Err(ParseError::BadContentLength), "{bad:?}");
    }
}

#[test]
fn line_ends() {
    let bare = INVITE.replace("\r\n", "\n");
    assert_eq!(
        Message::parse(bare.as_bytes()),
        Message::parse(INVITE.as_bytes())
    );
    // Line ends before the start line are ignored (section 7.5).
    let padded = format!("\r\n\r\n{INVITE}");
    assert_eq!(
        Message::parse(padded.as_bytes()),
        Message::parse(INVITE.as_bytes())
    );

    assert_eq!(Message::parse(b""), Err(ParseError::Empty));
    assert_eq!(Message::parse(b"\r\n\r\n"), Err(ParseError::Empty));
    assert_eq!(
        Message::parse(b"OPTIONS sip:a@example.com SIP/2.0\r\nVia: x\r\n"),
        Err(ParseError::NoEndOfHeaders)
    );
    assert_eq!(
        Message::parse(b"OPTIONS sip:a@example.com SIP/2.0"),
        Err(ParseError::NoEndOfHeaders)
    );
    assert_eq!(
        Message::parse(b"OPTIONS sip:a@example.com SIP/2.0\r\nSubject: a\rb\r\n\r\n"),
        Err(ParseError::BadHeader)
    );
}

#[test]
fn header_lines_that_are_not() {
    let parse = |line: &str| {
        Message::parse(format!("OPTIONS sip:a@example.com SIP/2.0\r\n{line}\r\n\r\n").as_bytes())
            .map(|_| ())
    };
    assert_eq!(parse("no colon here"), Err(ParseError::BadHeader));
    assert_eq!(parse(": no name"), Err(ParseError::BadHeader));
    assert_eq!(parse("two words: x"), Err(ParseError::BadHeader));
    assert_eq!(parse("Na(me: x"), Err(ParseError::BadHeader));
    assert_eq!(parse("Subject \t : spaces before the colon"), Ok(()));
    assert_eq!(parse("Subject:no space after it"), Ok(()));
    assert_eq!(parse("Subject:"), Ok(()));
    assert_eq!(parse("Subject: \u{00e9}t\u{00e9}"), Ok(()));
    assert_eq!(
        Message::parse(b"OPTIONS sip:a@example.com SIP/2.0\r\nSubject: \xff\r\n\r\n"),
        Err(ParseError::NotUtf8)
    );
}

#[test]
fn the_limits() {
    let head = "OPTIONS sip:a@example.com SIP/2.0\r\n";

    // 128 headers, and Content-Length is not one of them.
    let mut most = head.to_owned();
    for n in 0..MAX_HEADERS {
        most.push_str(&format!("X-{n}: {n}\r\n"));
    }
    let with_length = format!("{most}Content-Length: 0\r\n\r\n");
    assert_eq!(
        Message::parse(with_length.as_bytes())
            .unwrap()
            .headers()
            .0
            .len(),
        MAX_HEADERS
    );
    let over = format!("{most}X-More: 1\r\n\r\n");
    assert_eq!(
        Message::parse(over.as_bytes()),
        Err(ParseError::TooManyHeaders)
    );
    // Lines of folding are bounded too.
    let folds = format!(
        "{head}Subject: a\r\n{}\r\n",
        " b\r\n".repeat(5 * MAX_HEADERS)
    );
    assert_eq!(
        Message::parse(folds.as_bytes()),
        Err(ParseError::TooManyHeaders)
    );

    // A line of 8192 octets, and one more.
    let value = "v".repeat(MAX_LINE - "Subject: ".len());
    let fits = format!("{head}Subject: {value}\r\n\r\n");
    assert!(Message::parse(fits.as_bytes()).is_ok());
    let long = format!("{head}Subject: {value}v\r\n\r\n");
    assert_eq!(
        Message::parse(long.as_bytes()),
        Err(ParseError::LineTooLong)
    );
    // A header folded over short lines is still one line.
    let folded = format!(
        "{head}Subject: a\r\n{}\r\n",
        format!(" {}\r\n", "b".repeat(4000)).repeat(3)
    );
    assert_eq!(
        Message::parse(folded.as_bytes()),
        Err(ParseError::LineTooLong)
    );
    let long_uri = format!(
        "OPTIONS sip:{}@example.com SIP/2.0\r\n\r\n",
        "a".repeat(MAX_LINE)
    );
    assert_eq!(
        Message::parse(long_uri.as_bytes()),
        Err(ParseError::LineTooLong)
    );

    // A datagram of 65,535 octets, and one more.
    let mut datagram = format!("{head}\r\n").into_bytes();
    datagram.resize(MAX_DATAGRAM, b'x');
    assert_eq!(
        Message::parse(&datagram).unwrap().body().len(),
        MAX_DATAGRAM - head.len() - 2
    );
    datagram.push(b'x');
    assert_eq!(Message::parse(&datagram), Err(ParseError::TooLong));
}

#[test]
fn what_a_message_must_have() {
    let full = INVITE;
    let without = |name: &str| {
        let text: String = full
            .split_inclusive("\r\n")
            .filter(|line| !line.starts_with(name))
            .collect();
        request(&text).essentials()
    };
    assert_eq!(without("Via").unwrap_err(), Invalid::Header("Via"));
    assert_eq!(without("From").unwrap_err(), Invalid::Header("From"));
    assert_eq!(without("To").unwrap_err(), Invalid::Header("To"));
    assert_eq!(without("Call-ID").unwrap_err(), Invalid::Header("Call-ID"));
    assert_eq!(without("CSeq").unwrap_err(), Invalid::Header("CSeq"));
    // These are not needed to answer.
    assert!(without("Max-Forwards").is_ok());
    assert!(without("Contact").is_ok());

    let wrong = request(&full.replace("CSeq: 101 INVITE", "CSeq: 101 BYE"));
    assert_eq!(wrong.essentials().unwrap_err(), Invalid::CSeqMethod);
    let spaced = request(&full.replace("Call-ID: 3b1c9e@10.10.20.11", "Call-ID: two words"));
    assert_eq!(spaced.essentials().unwrap_err(), Invalid::Header("Call-ID"));
    // One Via that is not valid spoils the route back.
    let via = request(&full.replace("Max-Forwards: 70", "Via: SIP/2.0/UDP"));
    assert_eq!(via.essentials().unwrap_err(), Invalid::Header("Via"));
    assert_eq!(
        Invalid::Header("To").to_string(),
        "the To header is missing or not valid"
    );
}

#[test]
fn building_messages() {
    let mut request = Request::new(
        Method::Bye,
        &SipUri::new(Some("201"), "10.10.20.11", Some(5060)),
    );
    request
        .headers
        .push("v", "SIP/2.0/UDP 10.10.20.5;branch=z9hG4bKq");
    request.headers.push("call-id", "abc");
    request.headers.push("Content-Length", "999");
    request.headers.push("X-Note", "line one\r\nInjected: yes");
    assert_eq!(
        String::from_utf8(request.to_bytes()).unwrap(),
        "BYE sip:201@10.10.20.11:5060 SIP/2.0\r\n\
Via: SIP/2.0/UDP 10.10.20.5;branch=z9hG4bKq\r\n\
Call-ID: abc\r\n\
X-Note: line one  Injected: yes\r\n\
Content-Length: 0\r\n\
\r\n"
    );

    let mut response = Response::new(486);
    response.body = b"busy".to_vec();
    assert_eq!(
        response.to_bytes(),
        b"SIP/2.0 486 Busy Here\r\nContent-Length: 4\r\n\r\nbusy"
    );
    assert_eq!(reason_phrase(200), "OK");
    assert_eq!(reason_phrase(487), "Request Terminated");
    assert_eq!(reason_phrase(499), "Request Failure");
    assert_eq!(reason_phrase(199), "Progress");
    assert_eq!(reason_phrase(699), "Global Failure");
}

#[test]
fn editing_headers() {
    let mut headers = Headers::new();
    headers.push("Via", "one");
    headers.push("Subject", "old");
    headers.push("Via", "two");
    headers.push_front("v", "zero");
    assert_eq!(
        headers.all("Via").collect::<Vec<_>>(),
        ["zero", "one", "two"]
    );
    headers.set("s", "new");
    assert_eq!(headers.get("Subject"), Some("new"));
    assert_eq!(headers.0[2].name, "Subject");
    headers.set("Via", "only");
    assert_eq!(headers.all("Via").collect::<Vec<_>>(), ["only"]);
    assert_eq!(headers.0[0].name, "Via");
    headers.set("Expires", "300");
    assert_eq!(headers.expires(), Some(300));
    headers.remove("via");
    assert!(!headers.has("Via"));
    headers.set("Min-Expires", " 3600 ");
    assert_eq!(headers.min_expires(), Some(3600));
    headers.set("Expires", "1000000000000000000");
    assert_eq!(headers.expires(), None);
}
