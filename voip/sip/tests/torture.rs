// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Torture tests in the style of RFC 4475: the cases of that document that
//! apply to a UDP user agent, written afresh. Each test names the section
//! of RFC 4475 whose point it makes.
//!
//! The messages are written with bare line feeds and sent with CRLF.

use sip::{Contacts, Invalid, Message, Method, ParseError, Request, Response, UriError};

fn wire(text: &str) -> Vec<u8> {
    text.replace('\n', "\r\n").into_bytes()
}

fn request(text: &str) -> Request {
    match Message::parse(&wire(text)) {
        Ok(Message::Request(request)) => request,
        other => panic!("not a request: {other:?}"),
    }
}

fn response(text: &str) -> Response {
    match Message::parse(&wire(text)) {
        Ok(Message::Response(response)) => response,
        other => panic!("not a response: {other:?}"),
    }
}

fn refused(text: &str) -> ParseError {
    Message::parse(&wire(text)).expect_err("the message is refused")
}

/// The headers a request needs, for the cases that are about something
/// else.
const REST: &str = "Via: SIP/2.0/UDP 192.0.2.9;branch=z9hG4bKkdjuw
Max-Forwards: 70
From: <sip:caller@example.org>;tag=3824
To: <sip:line7@dial-in.example.net>
Call-ID: tort.20261126@192.0.2.9
";

/// 3.1.1.1: a short tortuous INVITE. Folding everywhere it may be, white
/// space around every separator, mixed case and compact names, escaped
/// quotes, unknown headers and parameters, and octets after the body.
#[test]
fn a_short_tortuous_invite() {
    let r = request(
        r#"INVITE sip:line7@dial-in.example.net;unknownparam SIP/2.0
TO :
 sip:line7@dial-in.example.net ;   tag    = 44910n
from   : "Q Operator \\\""       <sip:op@example.org>
  ;
  tag = 77abc
MaX-fOrWaRdS: 0068
Call-ID: tort.1234@192.0.2.9
Content-Length   : 5
cseq: 0009
  INVITE
Via  : SIP  /   2.0
 /UDP
    192.0.2.9;branch=390skdjuw
s :
NewFangledHeader:   newfangled value
 continued newfangled value
UnknownHeaderWithUnusualValue: ;;,,;;,;
Content-Type: application/sdp
Route:
 <sip:services.example.org;lr;unknownwith=value;unknown-no-value>
v:  SIP  / 2.0  / TCP     relay.example.org   ;
  branch  =   z9hG4bK9ikj8  ,
 SIP  /    2.0   / UDP  192.0.2.77   ; branch=
 z9hG4bK30239
m:"Quoted string \"\"" <sip:op@example.org> ; newparam =
      newvalue ;
  secondparam ; q = 0.33

v=0
these octets are after the body"#,
    );
    assert_eq!(r.method, Method::Invite);
    let uri = r.sip_uri().unwrap();
    assert_eq!(uri.user.as_deref(), Some("line7"));
    assert!(uri.params.has("unknownparam"));

    let to = r.headers.to().unwrap();
    assert_eq!(to.uri, "sip:line7@dial-in.example.net");
    assert_eq!(to.tag(), Some("44910n"));
    let from = r.headers.from().unwrap();
    assert_eq!(from.display.as_deref(), Some("Q Operator \\\""));
    assert_eq!(from.tag(), Some("77abc"));
    assert_eq!(r.headers.max_forwards(), Some(68));
    assert_eq!(r.headers.call_id(), Some("tort.1234@192.0.2.9"));
    let cseq = r.headers.cseq().unwrap();
    assert_eq!((cseq.sequence, cseq.method.as_str()), (9, "INVITE"));

    let vias = r.headers.vias().unwrap();
    let seen: Vec<_> = vias
        .iter()
        .map(|v| (v.transport.as_str(), v.host.as_str(), v.branch()))
        .collect();
    assert_eq!(
        seen,
        [
            ("UDP", "192.0.2.9", Some("390skdjuw")),
            ("TCP", "relay.example.org", Some("z9hG4bK9ikj8")),
            ("UDP", "192.0.2.77", Some("z9hG4bK30239")),
        ]
    );

    assert_eq!(r.headers.get("Subject"), Some(""));
    assert_eq!(
        r.headers.get("NewFangledHeader"),
        Some("newfangled value continued newfangled value")
    );
    assert_eq!(
        r.headers.get("UnknownHeaderWithUnusualValue"),
        Some(";;,,;;,;")
    );
    let route = r.headers.routes().unwrap();
    let route = route[0].sip_uri().unwrap();
    assert!(route.params.has("lr"));
    assert_eq!(route.params.get("unknownwith"), Some("value"));
    assert!(route.params.has("unknown-no-value"));

    let contact = r.headers.contact().unwrap();
    assert_eq!(contact.display.as_deref(), Some("Quoted string \"\""));
    assert_eq!(contact.params.get("newparam"), Some("newvalue"));
    assert!(contact.params.has("secondparam"));
    assert_eq!(contact.params.get("q"), Some("0.33"));

    assert_eq!(r.body, b"v=0\r\n");
    assert!(r.essentials().is_ok());
}

/// 3.1.1.2: a wide range of valid characters, in the method, in header
/// names and in values.
#[test]
fn a_wide_range_of_valid_characters() {
    let r = request(
        r#"!interesting-Method0123456789_*+`.%indeed'~ sip:1_unusual.INVITE@example.net SIP/2.0
Via: SIP/2.0/UDP host1.example.org;branch=z9hG4bK-.!%66*_+`'~
To: "BEL: NUL: DEL:" <sip:1_unusual.INVITE@example.net>
From: token1~` token2'+_ token3*%!.- <sip:mundane@example.org>;fromParam''~+*_!.-%="rab";tag=_token~1'+`*%!-.
Call-ID: intmeth.word%ZK-!.*_+'@word`~)(><:\/"][?}{
CSeq: 139122385 !interesting-Method0123456789_*+`.%indeed'~
Max-Forwards: 255
extensionHeader-!.%*+_`'~: value
Content-Length: 0

"#,
    );
    let method = "!interesting-Method0123456789_*+`.%indeed'~";
    assert_eq!(r.method, Method::Other(method.to_owned()));
    let e = r.essentials().unwrap();
    assert_eq!(e.cseq.method, method);
    assert_eq!(e.via.branch(), Some("z9hG4bK-.!%66*_+`'~"));
    assert_eq!(
        e.from.display.as_deref(),
        Some("token1~` token2'+_ token3*%!.-")
    );
    assert_eq!(e.from.tag(), Some("_token~1'+`*%!-."));
    assert_eq!(e.from.params.get("fromParam''~+*_!.-%"), Some("\"rab\""));
    assert_eq!(e.to.display.as_deref(), Some("BEL: NUL: DEL:"));
    assert_eq!(e.call_id, r#"intmeth.word%ZK-!.*_+'@word`~)(><:\/"][?}{"#);
    assert_eq!(r.headers.get("extensionHeader-!.%*+_`'~"), Some("value"));
}

/// 3.1.1.3 and 3.1.1.4: percent escapes in URIs are kept as written, an
/// escaped NUL included, and undone only when the user part is read.
#[test]
fn escapes_in_uris() {
    let r = request(&format!(
        "INVITE sip:sips%3Auser%40example.org@dial-in.example.net;lr SIP/2.0
{REST}CSeq: 1 INVITE
Contact: <sip:null-%00-null@example.org>
Route: <sip:relay.example.org;transport=u%64p>

"
    ));
    let uri = r.sip_uri().unwrap();
    assert_eq!(uri.user.as_deref(), Some("sips%3Auser%40example.org"));
    assert_eq!(uri.user_decoded().as_deref(), Some("sips:user@example.org"));
    let contact = r.headers.contact().unwrap().sip_uri().unwrap();
    assert_eq!(contact.user_decoded().as_deref(), Some("null-\0-null"));
    let route = r.headers.routes().unwrap()[0].sip_uri().unwrap();
    assert_eq!(route.params.get("transport"), Some("u%64p"));
}

/// 3.1.1.5: a "%" where no escape is defined is a character. This method
/// is not REGISTER.
#[test]
fn a_percent_sign_that_is_not_an_escape() {
    let r = request(&format!(
        "RE%47IST%45R sip:registrar.example.net SIP/2.0
{REST}CSeq: 1 RE%47IST%45R

"
    ));
    assert_eq!(r.method, Method::Other("RE%47IST%45R".to_owned()));
    assert_ne!(r.method, Method::Register);
    assert!(r.essentials().is_ok());
}

/// 3.1.1.6: no white space between a display name and the "<".
#[test]
fn no_space_between_the_display_name_and_the_bracket() {
    let r = request(
        "OPTIONS sip:line7@dial-in.example.net SIP/2.0
Via: SIP/2.0/UDP 192.0.2.9;branch=z9hG4bKkdjuw
From: caller<sip:caller@example.org>;tag=323
To: sip:line7@dial-in.example.net
Call-ID: lws.1@192.0.2.9
CSeq: 1 OPTIONS

",
    );
    let e = r.essentials().unwrap();
    assert_eq!(e.from.display.as_deref(), Some("caller"));
    assert_eq!(e.from.uri, "sip:caller@example.org");
    assert_eq!(e.from.tag(), Some("323"));
}

/// 3.1.1.7: long values: a long display name, many Vias, a long branch
/// and an unknown header that is longer still.
#[test]
fn long_values() {
    let name = "I have a user name of extreme proportion ".repeat(12);
    let branch = format!("z9hG4bK{}", "0123456789abcdef".repeat(16));
    let mut vias = format!("Via: SIP/2.0/UDP sip33.example.org;branch={branch}\n");
    for n in (1..33).rev() {
        vias.push_str(&format!("v: SIP/2.0/UDP sip{n}.example.org\n"));
    }
    let extra = "unknown-".repeat(700);
    let r = request(&format!(
        "INVITE sip:line7@dial-in.example.net SIP/2.0
To: \"{name}\" <sip:line7@dial-in.example.net:6000;unknownparam1=very{0}longvalue>
From: <sip:caller@example.org>;tag=12{1}982
Call-ID: longreq.one{2}really.long.call.id@192.0.2.9
CSeq: 3882340 INVITE
Unknown-{3}-Long-Name: {extra}
{vias}Max-Forwards: 70
Contact: <sip:{4}@192.0.2.9>

",
        "long".repeat(20),
        "9".repeat(200),
        ".long".repeat(40),
        "Long".repeat(30),
        "amazinglylongcallernameamazinglylongcallername".repeat(4),
    ));
    let e = r.essentials().unwrap();
    assert_eq!(e.to.display.as_deref(), Some(name.as_str()));
    assert_eq!(e.via.branch(), Some(branch.as_str()));
    assert_eq!(r.headers.vias().unwrap().len(), 33);
    assert_eq!(e.from.tag().unwrap().len(), 205);
    assert_eq!(r.headers.0.len(), 5 + 33 + 2);
}

/// 3.1.1.8: octets after the body are not part of the message over UDP.
#[test]
fn extra_octets_after_the_body() {
    let r = request(&format!(
        "REGISTER sip:registrar.example.net SIP/2.0
{REST}CSeq: 1 REGISTER
Content-Length: 0

INVITE sip:someone@example.net SIP/2.0
Via: SIP/2.0/UDP 192.0.2.66

"
    ));
    assert_eq!(r.method, Method::Register);
    assert!(r.body.is_empty());
    assert_eq!(r.headers.vias().unwrap().len(), 1);
}

/// 3.1.1.9: parameters in the user part of a URI belong to the user part.
#[test]
fn semicolons_in_the_user_part() {
    let r = request(&format!(
        "OPTIONS sip:user;par=u%40example.net@dial-in.example.net SIP/2.0
{REST}CSeq: 1 OPTIONS

"
    ));
    let uri = r.sip_uri().unwrap();
    assert_eq!(uri.user.as_deref(), Some("user;par=u%40example.net"));
    assert_eq!(uri.host, "dial-in.example.net");
    assert!(uri.params.is_empty());
}

/// 3.1.1.10: transports we have never heard of are still Vias.
#[test]
fn varied_and_unknown_transports() {
    let r = request(
        "OPTIONS sip:line7@dial-in.example.net SIP/2.0
Via: SIP/2.0/UDP t1.example.org;branch=z9hG4bKa
Via: SIP/2.0/SCTP t2.example.org;branch=z9hG4bKb
Via: SIP/2.0/TLS t3.example.org;branch=z9hG4bKc
Via: SIP/2.0/UNKNOWN t4.example.org;branch=z9hG4bKd
Via: SIP/2.0/TCP t5.example.org;branch=z9hG4bKe
From: <sip:caller@example.org>;tag=1
To: <sip:line7@dial-in.example.net>
Call-ID: transports.1@192.0.2.9
CSeq: 1 OPTIONS

",
    );
    let transports: Vec<String> = r
        .headers
        .vias()
        .unwrap()
        .into_iter()
        .map(|v| v.transport)
        .collect();
    assert_eq!(transports, ["UDP", "SCTP", "TLS", "UNKNOWN", "TCP"]);
}

/// 3.1.1.11: a body of several parts, some of them binary, is octets to
/// the parser.
#[test]
fn a_body_that_is_not_text() {
    let mut datagram = wire(&format!(
        "MESSAGE sip:line7@dial-in.example.net SIP/2.0
{REST}CSeq: 1 MESSAGE
Content-Type: multipart/mixed;boundary=7a9cbec02ceef655
Content-Length: 12

"
    ));
    datagram.extend_from_slice(&[
        0x30, 0x82, 0x01, 0x52, 0x00, 0xFF, b'\r', b'\n', b'-', b'-', 0x80, 0x00,
    ]);
    let Ok(Message::Request(r)) = Message::parse(&datagram) else {
        panic!("not a request");
    };
    assert_eq!(r.body.len(), 12);
    assert_eq!(r.headers.content_type().as_deref(), Some("multipart/mixed"));
}

/// 3.1.1.12 and 3.1.1.13: a reason phrase in any characters, and none.
#[test]
fn unusual_reason_phrases() {
    let rest = "Via: SIP/2.0/UDP 192.0.2.9;branch=z9hG4bKkdjuw
From: <sip:caller@example.org>;tag=3824
To: <sip:line7@dial-in.example.net>;tag=9
Call-ID: reason.1@192.0.2.9
CSeq: 1 INVITE

";
    let r = response(&format!(
        "SIP/2.0 200 = 2**3 * 5**2 \u{043d}\u{043e} \u{0441}\u{0442}\u{043e} \u{0434}\u{0435}\u{0432}\u{044f}\u{043d}\u{043e}\u{0441}\u{0442}\u{043e}\n{rest}"
    ));
    assert_eq!(r.status, 200);
    assert!(
        r.reason
            .ends_with("\u{0434}\u{0435}\u{0432}\u{044f}\u{043d}\u{043e}\u{0441}\u{0442}\u{043e}")
    );
    assert!(r.essentials().is_ok());

    let r = response(&format!("SIP/2.0 100 \n{rest}"));
    assert_eq!((r.status, r.reason.as_str()), (100, ""));
    assert!(r.essentials().is_ok());
}

/// 3.1.2.1: separators with nothing between them. Empty parameters are
/// read past. A Via list with elements that are only separators is not a
/// list of Vias, and the request cannot be answered.
#[test]
fn extraneous_separators() {
    let rest = "From: <sip:caller@example.org>;tag=1
To: <sip:line7@dial-in.example.net>
Call-ID: badinv.1@192.0.2.9
CSeq: 1 INVITE
Contact: \"Joe\" <sip:joe@example.org>;;;;

";
    let r = request(&format!(
        "INVITE sip:line7@dial-in.example.net SIP/2.0\nVia: SIP/2.0/UDP 192.0.2.9;;branch=z9hG4bKa;;,,\n{rest}"
    ));
    assert_eq!(r.headers.vias().unwrap().len(), 1);
    assert!(r.headers.contact().unwrap().params.is_empty());
    assert!(r.essentials().is_ok());

    let r = request(&format!(
        "INVITE sip:line7@dial-in.example.net SIP/2.0\nVia: SIP/2.0/UDP 192.0.2.9;;,;,,\n{rest}"
    ));
    assert_eq!(r.headers.vias(), None);
    assert_eq!(r.essentials().unwrap_err(), Invalid::Header("Via"));
}

/// 3.1.2.2 and 3.1.2.3: a Content-Length larger than the datagram, and a
/// negative one. Neither is a buffer size.
#[test]
fn content_lengths_that_lie() {
    assert_eq!(
        refused(&format!(
            "INVITE sip:line7@dial-in.example.net SIP/2.0
{REST}CSeq: 1 INVITE
Content-Length: 9999

v=0
"
        )),
        ParseError::Truncated
    );
    assert_eq!(
        refused(&format!(
            "INVITE sip:line7@dial-in.example.net SIP/2.0
{REST}CSeq: 1 INVITE
Content-Length: -999

v=0
"
        )),
        ParseError::BadContentLength
    );
    assert_eq!(
        refused(&format!(
            "INVITE sip:line7@dial-in.example.net SIP/2.0
{REST}CSeq: 1 INVITE
Content-Length: 18446744073709551617

"
        )),
        ParseError::BadContentLength
    );
}

/// 3.1.2.4 and 3.1.2.5: numbers that overflow, in a request and in a
/// response. The message parses; the fields are not numbers.
#[test]
fn scalar_fields_with_overlarge_values() {
    let r = request(
        "REGISTER sip:registrar.example.net SIP/2.0
Via: SIP/2.0/UDP 192.0.2.9:5060;branch=z9hG4bKkdjuw
From: <sip:caller@example.org>;tag=3824
To: <sip:caller@example.org>
Call-ID: scalar.1@192.0.2.9
CSeq: 36893488147419103232 REGISTER
Max-Forwards: 300
Expires: 1000000000000000000
Contact: <sip:caller@192.0.2.9>;expires=280297596632815

",
    );
    assert_eq!(r.headers.cseq(), None);
    assert_eq!(r.essentials().unwrap_err(), Invalid::Header("CSeq"));
    assert_eq!(r.headers.expires(), None);
    assert_eq!(r.headers.max_forwards(), Some(300));
    assert_eq!(
        r.headers.contact().unwrap().params.get("expires"),
        Some("280297596632815")
    );

    assert_eq!(
        refused(
            "SIP/2.0 4294967301 better not break the receiver
Via: SIP/2.0/UDP 192.0.2.9;branch=z9hG4bKkdjuw
CSeq: 1 INVITE

"
        ),
        ParseError::BadStartLine
    );
    let r = response(
        "SIP/2.0 503 Service Unavailable
Via: SIP/2.0/UDP 192.0.2.9;branch=z9hG4bKkdjuw
From: <sip:caller@example.org>;tag=3824
To: <sip:line7@dial-in.example.net>;tag=1
Call-ID: scalar.2@192.0.2.9
CSeq: 9292394834772304023312 OPTIONS
Retry-After: 949302838503028349304023988

",
    );
    assert_eq!(r.essentials().unwrap_err(), Invalid::Header("CSeq"));
}

/// 3.1.2.6: a quoted string that does not end.
#[test]
fn an_unterminated_quoted_string() {
    let r = request(
        "INVITE sip:line7@dial-in.example.net SIP/2.0
Via: SIP/2.0/UDP 192.0.2.9;branch=z9hG4bKkdjuw
From: <sip:caller@example.org>;tag=3824
To: \"Mr. J. User <sip:line7@dial-in.example.net>
Call-ID: quotbal.1@192.0.2.9
CSeq: 1 INVITE

",
    );
    assert_eq!(r.headers.to(), None);
    assert_eq!(r.essentials().unwrap_err(), Invalid::Header("To"));
}

/// 3.1.2.7 to 3.1.2.10: a Request-URI in angle brackets, with white space
/// in it, with more than one space around it, and with spaces after the
/// version.
#[test]
fn request_lines_that_are_not() {
    for line in [
        "INVITE <sip:line7@dial-in.example.net> SIP/2.0",
        "INVITE sip:line7@dial-in.example.net; lr SIP/2.0",
        "INVITE  sip:line7@dial-in.example.net  SIP/2.0",
        "OPTIONS sip:line7@dial-in.example.net SIP/2.0   ",
    ] {
        assert_eq!(
            refused(&format!("{line}\n{REST}CSeq: 1 INVITE\n\n")),
            ParseError::BadStartLine,
            "{line}"
        );
    }
}

/// 3.1.2.11: headers escaped into the Request-URI are part of the URI and
/// are not headers of the request.
#[test]
fn escaped_headers_in_the_request_uri() {
    let r = request(&format!(
        "INVITE sip:line7@dial-in.example.net?Route=%3Csip:example.org%3E SIP/2.0
{REST}CSeq: 1 INVITE

"
    ));
    assert!(r.headers.routes().unwrap().is_empty());
    assert_eq!(
        r.sip_uri().unwrap().headers,
        [("Route".to_owned(), "%3Csip:example.org%3E".to_owned())]
    );
}

/// 3.1.2.13: a display name in quotes and a URI with no brackets.
#[test]
fn a_name_addr_without_brackets() {
    let r = request(&format!(
        "REGISTER sip:registrar.example.net SIP/2.0
{REST}CSeq: 1 REGISTER
Contact: \"Joe\" sip:joe@example.org

"
    ));
    assert_eq!(r.headers.contacts(), None);
    assert_eq!(r.headers.contact(), None);
}

/// 3.1.2.15: characters in a display name that are not token characters.
/// We only ever echo a From, so this is read as a name.
#[test]
fn non_token_characters_in_a_display_name() {
    let r = request(
        "OPTIONS sip:line7@dial-in.example.net SIP/2.0
Via: SIP/2.0/UDP 192.0.2.9;branch=z9hG4bKkdjuw
From: Bell, Alexander <sip:a.g.bell@example.org>;tag=43
To: <sip:line7@dial-in.example.net>
Call-ID: baddn.1@192.0.2.9
CSeq: 1 OPTIONS

",
    );
    let from = r.headers.from().unwrap();
    assert_eq!(from.display.as_deref(), Some("Bell, Alexander"));
    assert_eq!(from.tag(), Some("43"));
}

/// 3.1.2.16: a version we do not speak.
#[test]
fn an_unknown_protocol_version() {
    assert_eq!(
        refused(&format!(
            "OPTIONS sip:line7@dial-in.example.net SIP/7.0\n{REST}CSeq: 1 OPTIONS\n\n"
        )),
        ParseError::BadVersion
    );
    assert_eq!(
        refused(&format!("SIP/7.0 200 OK\n{REST}CSeq: 1 OPTIONS\n\n")),
        ParseError::BadVersion
    );
}

/// 3.1.2.17 and 3.1.2.18: the method of the start line is not the method
/// of CSeq, for a known method and for an unknown one.
#[test]
fn the_start_line_and_cseq_disagree() {
    for (line, cseq) in [
        ("OPTIONS", "INVITE"),
        ("NEWMETHOD", "INVITE"),
        ("INVITE", "invite"),
    ] {
        let r = request(&format!(
            "{line} sip:line7@dial-in.example.net SIP/2.0\n{REST}CSeq: 8 {cseq}\n\n"
        ));
        assert_eq!(r.essentials().unwrap_err(), Invalid::CSeqMethod, "{line}");
    }
}

/// 3.3.1: required headers are missing.
#[test]
fn missing_required_headers() {
    let r = request(
        "INVITE sip:line7@dial-in.example.net SIP/2.0
CSeq: 193942 INVITE
Via: SIP/2.0/UDP 192.0.2.9;branch=z9hG4bKkdjuw
Content-Type: application/sdp

",
    );
    assert_eq!(r.essentials().unwrap_err(), Invalid::Header("From"));
    let bare = request("OPTIONS sip:line7@dial-in.example.net SIP/2.0\n\n");
    assert_eq!(bare.essentials().unwrap_err(), Invalid::Header("Via"));
}

/// 3.3.2 to 3.3.5: URI schemes we do not take: one nobody knows, in the
/// Request-URI and in a header, and the ones we know and refuse.
#[test]
fn unknown_and_refused_uri_schemes() {
    for uri in [
        "nobodyKnowsThisScheme:totallyopaquecontent",
        "soap.beep://192.0.2.103:3002",
        "http://www.example.com/",
        "sips:line7@dial-in.example.net",
        "tel:+15551000",
    ] {
        let r = request(&format!("OPTIONS {uri} SIP/2.0\n{REST}CSeq: 1 OPTIONS\n\n"));
        assert_eq!(r.sip_uri(), Err(UriError::UnsupportedScheme), "{uri}");
        assert!(r.essentials().is_ok(), "{uri}: it can be answered");
    }

    let r = request(
        "REGISTER sip:registrar.example.net SIP/2.0
Via: SIP/2.0/UDP 192.0.2.9;branch=z9hG4bKkdjuw
From: <sip:caller@example.org>;tag=3824
To: isbn:2983792873
Call-ID: unksm2.1@192.0.2.9
CSeq: 1 REGISTER
Contact: <name:John_Smith>

",
    );
    let e = r.essentials().unwrap();
    assert_eq!(e.to.uri, "isbn:2983792873");
    assert_eq!(e.to.sip_uri(), Err(UriError::UnsupportedScheme));
    assert!(matches!(r.headers.contacts(), Some(Contacts::List(list)) if list.len() == 1));
}

/// 3.3.8 and 3.3.9: Content-Length given twice with two values is
/// refused; the other headers that should be single are read as their
/// first value.
#[test]
fn headers_that_should_be_single_given_twice() {
    assert_eq!(
        refused(&format!(
            "OPTIONS sip:line7@dial-in.example.net SIP/2.0
{REST}CSeq: 1 OPTIONS
Content-Length: 0
l: 4

abcd"
        )),
        ParseError::BadContentLength
    );
    let r = request(&format!(
        "OPTIONS sip:line7@dial-in.example.net SIP/2.0
{REST}Call-ID: second@192.0.2.9
CSeq: 1 OPTIONS
CSeq: 2 OPTIONS
To: <sip:other@example.net>

"
    ));
    let e = r.essentials().unwrap();
    assert_eq!(e.call_id, "tort.20261126@192.0.2.9");
    assert_eq!(e.cseq.sequence, 1);
    assert_eq!(e.to.uri, "sip:line7@dial-in.example.net");
}

/// 3.3.10 and 3.3.11: Require and unknown extensions are there to be
/// answered with 420, not to stop the parser.
#[test]
fn required_extensions_are_reported() {
    let r = request(&format!(
        "OPTIONS sip:line7@dial-in.example.net SIP/2.0
{REST}CSeq: 1 OPTIONS
Require: nothingSupportsThis, nothingSupportsThisEither
Proxy-Require: noProxiesSupportThis

"
    ));
    assert_eq!(
        r.headers.require(),
        ["nothingSupportsThis", "nothingSupportsThisEither"]
    );
    assert_eq!(r.headers.get("Proxy-Require"), Some("noProxiesSupportThis"));
}

/// 3.3.15: headers with nothing in them.
#[test]
fn headers_with_no_value() {
    let r = request(&format!(
        "OPTIONS sip:line7@dial-in.example.net SIP/2.0
{REST}CSeq: 1 OPTIONS
Accept:
Allow:
Supported:
Require:
Expires:
Contact:

"
    ));
    assert!(r.headers.allow().is_empty());
    assert!(r.headers.supported().is_empty());
    assert!(r.headers.require().is_empty());
    assert_eq!(r.headers.expires(), None);
    assert_eq!(r.headers.contacts(), Some(Contacts::List(Vec::new())));
    assert!(r.essentials().is_ok());
}

/// Not text at all.
#[test]
fn octets_that_are_not_a_message() {
    for datagram in [
        &[0u8; 64][..],
        &[0xFFu8; 64][..],
        b"\x80\x00\x12\x34\x89\xAB\xCD\xEF\xDE\xAD\xBE\xEF payload",
        b"\r\n",
        b"jaK\r\n\r\n",
        b"\x16\x03\x01\x02\x00\x01\x00\x01\xfc\x03\x03",
    ] {
        assert!(Message::parse(datagram).is_err(), "{datagram:?}");
    }
}
