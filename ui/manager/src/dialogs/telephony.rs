// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The SIP and Media pages of the settings.
//!
//! Each holds the keys of its table of the configuration, `[sip]` and
//! `[media]`, but for the trunk peers, which have a dialog of their own
//! under the SIP menu. A page sends what was changed on it and nothing
//! else.

use std::cell::Cell;
use std::rc::Rc;

use ctlproto::{ErrorInfo, MediaChange, MediaSettings, SettingsChange, SipChange, SipSettings};
use win32::controls::{self, Combo, UpDown};
use win32::{DialogTemplate, Event, Handler, Hwnd, Reply, code};

use super::sheet::Shared;
use super::{must_not_be_empty, number_in, watch, watched};

/// The ids of the controls of the two pages.
pub mod ids {
    /// SIP enabled.
    pub const ENABLED: u16 = 320;
    /// The address the listener binds.
    pub const LISTEN: u16 = 321;
    /// The address to advertise.
    pub const ADDRESS: u16 = 322;
    /// The User-Agent.
    pub const USER_AGENT: u16 = 323;
    /// The ring timeout.
    pub const RING: u16 = 324;
    /// The arrows beside the ring timeout.
    pub const RING_ARROWS: u16 = 325;
    /// The codecs.
    pub const CODECS: u16 = 330;
    /// The packet time.
    pub const PTIME: u16 = 331;
    /// The first RTP port.
    pub const FIRST_PORT: u16 = 332;
    /// The last RTP port.
    pub const LAST_PORT: u16 = 333;
    /// The RTP timeout.
    pub const TIMEOUT: u16 = 334;
    /// The arrows beside the RTP timeout.
    pub const TIMEOUT_ARROWS: u16 = 335;
    /// The capture directory.
    pub const CAPTURES: u16 = 336;
    /// Browse, beside the capture directory.
    pub const BROWSE: u16 = 337;
}

/// What the SIP page says under its fields.
const SIP_NOTE: &str = "Turning SIP on or off, and a new listen address, take effect when the \
                        service next starts. The rest takes effect for the calls that follow.";

/// What the Media page says under its fields.
const MEDIA_NOTE: &str = "New RTP ports and a new capture directory take effect when the \
                          service next starts. The rest takes effect for the calls that follow.";

/// The codec lists, in the order the combo box has them: what the user
/// is shown, and what the service is sent.
pub const CODECS: [(&str, &[&str]); 4] = [
    ("PCMU, then PCMA", &["pcmu", "pcma"]),
    ("PCMA, then PCMU", &["pcma", "pcmu"]),
    ("PCMU only", &["pcmu"]),
    ("PCMA only", &["pcma"]),
];

/// The packet times, in milliseconds.
pub const PTIMES: [u32; 3] = [10, 20, 30];

/// The longest a line may ring and a call may go without a packet, in
/// seconds, as the fields take them.
const SECONDS: (u32, u32) = (1, 3600);

pub(super) fn sip_template() -> DialogTemplate {
    DialogTemplate::new("", 240, 130)
        .check_box(ids::ENABLED, 7, 7, 226, 10, "&Speak SIP")
        .label(7, 25, 76, 8, "&Listen on:")
        .edit(ids::LISTEN, 86, 23, 100, 14)
        .label(7, 43, 76, 8, "&Advertise address:")
        .edit(ids::ADDRESS, 86, 41, 100, 14)
        .label(7, 61, 76, 8, "&User agent:")
        .edit(ids::USER_AGENT, 86, 59, 147, 14)
        .label(7, 79, 76, 8, "&Ring for (seconds):")
        .number(ids::RING, 86, 77, 40, 14)
        .up_down(ids::RING_ARROWS)
        .label(7, 98, 226, 24, SIP_NOTE)
}

pub(super) fn media_template() -> DialogTemplate {
    DialogTemplate::new("", 240, 130)
        .label(7, 9, 76, 8, "&Codecs:")
        .combo(ids::CODECS, 86, 7, 100, 70)
        .label(7, 27, 76, 8, "&Packet time:")
        .combo(ids::PTIME, 86, 25, 60, 60)
        .label(7, 45, 76, 8, "RTP p&orts:")
        .number(ids::FIRST_PORT, 86, 43, 36, 14)
        .label(126, 45, 10, 8, "to")
        .number(ids::LAST_PORT, 138, 43, 36, 14)
        .label(7, 63, 76, 8, "RTP &timeout (seconds):")
        .number(ids::TIMEOUT, 86, 61, 40, 14)
        .up_down(ids::TIMEOUT_ARROWS)
        .label(7, 81, 76, 8, "Capture &directory:")
        .edit(ids::CAPTURES, 86, 79, 98, 14)
        .button(ids::BROWSE, 188, 79, 45, 14, "B&rowse\u{2026}")
        .label(7, 100, 226, 24, MEDIA_NOTE)
}

/// What the SIP page holds, as it was typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SipForm {
    /// Whether the service speaks SIP.
    pub enabled: bool,
    /// The address the listener binds.
    pub listen: String,
    /// The address to advertise.
    pub address: String,
    /// The User-Agent.
    pub user_agent: String,
    /// The ring timeout in seconds.
    pub ring: String,
}

impl SipForm {
    /// What the page changes: only what is not as the service has it.
    pub fn change(&self, before: &SipSettings) -> Result<Option<SipChange>, ErrorInfo> {
        let listen = self.listen.trim();
        if listen.is_empty() {
            return Err(must_not_be_empty("Listen on"));
        }
        let ring = number_in("Ring for", &self.ring, SECONDS.0, SECONDS.1)?;
        let (address, user_agent) = (self.address.trim(), self.user_agent.trim());
        let change = SipChange {
            enabled: (self.enabled != before.enabled).then_some(self.enabled),
            listen: (listen != before.listen).then(|| listen.to_string()),
            address: (address != before.address).then(|| address.to_string()),
            user_agent: (user_agent != before.user_agent).then(|| user_agent.to_string()),
            ring_timeout_s: (ring != before.ring_timeout_s).then_some(ring),
            ..SipChange::default()
        };
        Ok((change != SipChange::default()).then_some(change))
    }
}

/// What the Media page holds, as it was typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaForm {
    /// The codecs chosen, as the service names them; `None` when the
    /// service has a list the combo box does not offer and it was left
    /// alone.
    pub codecs: Option<Vec<String>>,
    /// The packet time chosen.
    pub ptime_ms: Option<u32>,
    /// The first RTP port.
    pub first_port: String,
    /// The last RTP port.
    pub last_port: String,
    /// The RTP timeout in seconds.
    pub timeout: String,
    /// The capture directory.
    pub captures: String,
}

impl MediaForm {
    /// What the page changes: only what is not as the service has it.
    pub fn change(&self, before: &MediaSettings) -> Result<Option<MediaChange>, ErrorInfo> {
        let first = number_in("RTP ports", &self.first_port, 1024, 65535)?;
        let last = number_in("RTP ports", &self.last_port, first, 65535)?;
        let ports = format!("{first}-{last}");
        let timeout = number_in("RTP timeout", &self.timeout, SECONDS.0, SECONDS.1)?;
        let captures = self.captures.trim();
        let change = MediaChange {
            codecs: self
                .codecs
                .clone()
                .filter(|codecs| *codecs != before.codecs),
            ptime_ms: self.ptime_ms.filter(|ptime| *ptime != before.ptime_ms),
            rtp_ports: (ports != before.rtp_ports).then_some(ports),
            rtp_timeout_s: (timeout != before.rtp_timeout_s).then_some(timeout),
            capture_dir: (captures != before.capture_dir).then(|| captures.to_string()),
        };
        Ok((change != MediaChange::default()).then_some(change))
    }
}

/// The two ends of a port range as the service writes it.
fn port_ends(range: &str) -> (String, String) {
    match range.split_once('-') {
        Some((first, last)) => (first.to_string(), last.to_string()),
        None => (range.to_string(), range.to_string()),
    }
}

pub(super) struct SipPage {
    pub shared: Rc<Shared>,
    /// The page has been filled in: a change from here on is the user's.
    pub filled: Cell<bool>,
}

impl Handler for SipPage {
    fn event(&self, page: Hwnd, event: Event<'_>) -> Reply {
        match event {
            Event::InitDialog => {
                let settings = self.shared.settings.borrow();
                let sip = &settings.sip;
                controls::set_checked(page.item(ids::ENABLED), sip.enabled);
                page.item(ids::LISTEN).set_text(&sip.listen);
                page.item(ids::ADDRESS).set_text(&sip.address);
                page.item(ids::USER_AGENT).set_text(&sip.user_agent);
                let ring = page.item(ids::RING);
                UpDown::of(page.item(ids::RING_ARROWS)).attach(
                    ring,
                    SECONDS.0 as i32,
                    SECONDS.1 as i32,
                );
                ring.set_text(&sip.ring_timeout_s.to_string());
                self.filled.set(true);
                watch(&self.shared.manager, page);
                Reply::Default
            }
            Event::Timer { id } if watched(&self.shared.manager, page, id, "settings-sip") => {
                Reply::Handled
            }
            Event::Command {
                id: ids::ENABLED, ..
            }
            | Event::Command {
                id: ids::LISTEN | ids::ADDRESS | ids::USER_AGENT | ids::RING,
                code: code::EDIT_CHANGE,
            } => {
                if self.filled.get() {
                    page.page_changed();
                }
                Reply::Handled
            }
            Event::Apply => {
                let form = SipForm {
                    enabled: controls::checked(page.item(ids::ENABLED)),
                    listen: page.item(ids::LISTEN).text(),
                    address: page.item(ids::ADDRESS).text(),
                    user_agent: page.item(ids::USER_AGENT).text(),
                    ring: page.item(ids::RING).text(),
                };
                let change = form.change(&self.shared.settings.borrow().sip);
                match change {
                    Ok(Some(sip)) => self.shared.apply(
                        page,
                        SettingsChange {
                            sip: Some(sip),
                            ..SettingsChange::default()
                        },
                    ),
                    Ok(None) => Reply::Default,
                    Err(error) => self.shared.refuse(page, &error),
                }
            }
            _ => Reply::Default,
        }
    }
}

pub(super) struct MediaPage {
    pub shared: Rc<Shared>,
    /// The page has been filled in: a change from here on is the user's.
    pub filled: Cell<bool>,
}

impl Handler for MediaPage {
    fn event(&self, page: Hwnd, event: Event<'_>) -> Reply {
        match event {
            Event::InitDialog => {
                let settings = self.shared.settings.borrow();
                let media = &settings.media;
                let codecs = Combo::of(page.item(ids::CODECS));
                for (text, _) in CODECS {
                    codecs.add(text);
                }
                // A list the combo box does not offer, written by hand,
                // leaves it with nothing chosen and is left alone.
                codecs.select(CODECS.iter().position(|(_, names)| {
                    names
                        .iter()
                        .copied()
                        .eq(media.codecs.iter().map(String::as_str))
                }));
                let ptime = Combo::of(page.item(ids::PTIME));
                for ms in PTIMES {
                    ptime.add(&format!("{ms} ms"));
                }
                ptime.select(PTIMES.iter().position(|ms| *ms == media.ptime_ms));
                let (first, last) = port_ends(&media.rtp_ports);
                page.item(ids::FIRST_PORT).set_text(&first);
                page.item(ids::LAST_PORT).set_text(&last);
                let timeout = page.item(ids::TIMEOUT);
                UpDown::of(page.item(ids::TIMEOUT_ARROWS)).attach(
                    timeout,
                    SECONDS.0 as i32,
                    SECONDS.1 as i32,
                );
                timeout.set_text(&media.rtp_timeout_s.to_string());
                page.item(ids::CAPTURES).set_text(&media.capture_dir);
                self.filled.set(true);
                watch(&self.shared.manager, page);
                Reply::Default
            }
            Event::Timer { id } if watched(&self.shared.manager, page, id, "settings-media") => {
                Reply::Handled
            }
            Event::Command {
                id: ids::BROWSE, ..
            } => {
                let sheet = page.parent().unwrap_or(page);
                if let Some(folder) = self.shared.manager.pick_folder(sheet, "Capture directory") {
                    page.item(ids::CAPTURES)
                        .set_text(&folder.display().to_string());
                    page.page_changed();
                }
                Reply::Handled
            }
            Event::Command {
                id: ids::CODECS | ids::PTIME,
                code: code::COMBO_SELECTION,
            }
            | Event::Command {
                id: ids::FIRST_PORT | ids::LAST_PORT | ids::TIMEOUT | ids::CAPTURES,
                code: code::EDIT_CHANGE,
            } => {
                if self.filled.get() {
                    page.page_changed();
                }
                Reply::Handled
            }
            Event::Apply => {
                let form = MediaForm {
                    codecs: Combo::of(page.item(ids::CODECS))
                        .selected()
                        .and_then(|index| CODECS.get(index))
                        .map(|(_, names)| names.iter().map(|name| name.to_string()).collect()),
                    ptime_ms: Combo::of(page.item(ids::PTIME))
                        .selected()
                        .and_then(|index| PTIMES.get(index))
                        .copied(),
                    first_port: page.item(ids::FIRST_PORT).text(),
                    last_port: page.item(ids::LAST_PORT).text(),
                    timeout: page.item(ids::TIMEOUT).text(),
                    captures: page.item(ids::CAPTURES).text(),
                };
                let change = form.change(&self.shared.settings.borrow().media);
                match change {
                    Ok(Some(media)) => self.shared.apply(
                        page,
                        SettingsChange {
                            media: Some(media),
                            ..SettingsChange::default()
                        },
                    ),
                    Ok(None) => Reply::Default,
                    Err(error) => self.shared.refuse(page, &error),
                }
            }
            _ => Reply::Default,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sip() -> SipSettings {
        SipSettings {
            enabled: true,
            listen: "0.0.0.0:5060".to_string(),
            address: String::new(),
            user_agent: String::new(),
            trusted_peers: vec!["10.10.20.11".to_string()],
            outbound_peer: "10.10.20.11".to_string(),
            ring_timeout_s: 60,
        }
    }

    fn sip_form() -> SipForm {
        SipForm {
            enabled: true,
            listen: " 0.0.0.0:5060 ".to_string(),
            address: String::new(),
            user_agent: String::new(),
            ring: "60".to_string(),
        }
    }

    fn media() -> MediaSettings {
        MediaSettings {
            codecs: vec!["pcmu".to_string(), "pcma".to_string()],
            ptime_ms: 20,
            rtp_ports: "20000-20999".to_string(),
            rtp_timeout_s: 10,
            capture_dir: String::new(),
        }
    }

    fn media_form() -> MediaForm {
        MediaForm {
            codecs: Some(vec!["pcmu".to_string(), "pcma".to_string()]),
            ptime_ms: Some(20),
            first_port: "20000".to_string(),
            last_port: "20999".to_string(),
            timeout: "10".to_string(),
            captures: String::new(),
        }
    }

    #[test]
    fn a_page_that_was_not_touched_changes_nothing() {
        assert_eq!(sip_form().change(&sip()), Ok(None));
        assert_eq!(media_form().change(&media()), Ok(None));
    }

    #[test]
    fn only_what_was_changed_on_the_sip_page_is_sent() {
        let form = SipForm {
            enabled: false,
            address: "203.0.113.7".to_string(),
            ring: "30".to_string(),
            ..sip_form()
        };
        let change = form.change(&sip()).unwrap().unwrap();
        assert_eq!(change.enabled, Some(false));
        assert_eq!(change.address.as_deref(), Some("203.0.113.7"));
        assert_eq!(change.ring_timeout_s, Some(30));
        assert_eq!((change.listen, change.user_agent), (None, None));
        // The peers are another dialog's.
        assert_eq!((change.trusted_peers, change.outbound_peer), (None, None));

        let empty = SipForm {
            listen: "  ".to_string(),
            ..sip_form()
        };
        assert_eq!(
            empty.change(&sip()).unwrap_err().text,
            "Listen on must not be empty."
        );
        let forever = SipForm {
            ring: "0".to_string(),
            ..sip_form()
        };
        assert_eq!(forever.change(&sip()).unwrap_err().msgid, "RMD4008");
    }

    #[test]
    fn only_what_was_changed_on_the_media_page_is_sent() {
        let form = MediaForm {
            codecs: Some(vec!["pcma".to_string()]),
            ptime_ms: Some(10),
            last_port: "20099".to_string(),
            captures: r" D:\captures ".to_string(),
            ..media_form()
        };
        let change = form.change(&media()).unwrap().unwrap();
        assert_eq!(change.codecs, Some(vec!["pcma".to_string()]));
        assert_eq!(change.ptime_ms, Some(10));
        assert_eq!(change.rtp_ports.as_deref(), Some("20000-20099"));
        assert_eq!(change.capture_dir.as_deref(), Some(r"D:\captures"));
        assert_eq!(change.rtp_timeout_s, None);
    }

    #[test]
    fn the_ports_are_a_range_above_the_ones_the_system_keeps() {
        for (first, last) in [("80", "20999"), ("20000", "19999"), ("20000", "70000")] {
            let form = MediaForm {
                first_port: first.to_string(),
                last_port: last.to_string(),
                ..media_form()
            };
            let error = form.change(&media()).unwrap_err();
            assert_eq!(error.msgid, "RMD4008", "{first}-{last}");
        }
        assert_eq!(
            port_ends("20000-20999"),
            ("20000".to_string(), "20999".to_string())
        );
    }

    #[test]
    fn a_codec_list_the_page_does_not_offer_is_left_as_it_is() {
        // Written by hand in the file: the same codec twice.
        let odd = MediaSettings {
            codecs: vec!["pcmu".to_string(), "pcmu".to_string()],
            ..media()
        };
        let form = MediaForm {
            codecs: None,
            ptime_ms: None,
            ..media_form()
        };
        assert_eq!(form.change(&odd), Ok(None));
    }

    #[test]
    fn every_codec_list_and_packet_time_of_the_service_can_be_chosen() {
        assert_eq!(PTIMES, [10, 20, 30]);
        assert_eq!(CODECS[0].1, ["pcmu", "pcma"]);
        assert_eq!(CODECS.len(), 4);
    }
}
