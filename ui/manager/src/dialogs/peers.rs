// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Trunk peers: the addresses that may send calls to the service, and
//! where the lines that do not register send the calls they dial.
//!
//! Both are keys of `[sip]`. OK sends `config.set` with what was changed
//! and nothing else; the service checks it, and if it refuses, its
//! message is shown and the dialog stays open as it was.

use std::cell::RefCell;
use std::rc::Rc;

use ctlproto::{Call, ErrorInfo, Settings, SettingsChange, SettingsResult, SipChange, SipSettings};
use win32::{DialogTemplate, Event, Handler, Hwnd, ListBox, Reply, code, id};

use super::{must_not_be_empty, watch, watched};
use crate::window::Manager;

/// The ids of the dialog's controls.
pub mod ids {
    /// The list of trusted peers.
    pub const PEERS: u16 = 300;
    /// The address to add.
    pub const PEER: u16 = 301;
    /// Add.
    pub const ADD: u16 = 302;
    /// Remove.
    pub const REMOVE: u16 = 303;
    /// The outbound peer.
    pub const OUTBOUND: u16 = 304;
}

use ids::{
    ADD as ID_ADD, OUTBOUND as ID_OUTBOUND, PEER as ID_PEER, PEERS as ID_PEERS, REMOVE as ID_REMOVE,
};

fn template() -> DialogTemplate {
    DialogTemplate::new("Trunk peers", 250, 206)
        .group(7, 7, 236, 122, "Trusted peers")
        .label(
            14,
            20,
            222,
            16,
            "The addresses, or blocks of addresses, that may send calls to the service. \
             A request from any other is dropped.",
        )
        .list_box(ID_PEERS, 14, 40, 168, 62)
        .button(ID_REMOVE, 186, 40, 50, 14, "&Remove")
        .label(14, 110, 34, 8, "A&ddress:")
        .edit(ID_PEER, 50, 108, 132, 14)
        .button(ID_ADD, 186, 108, 50, 14, "&Add")
        .group(7, 133, 236, 46, "Outbound peer")
        .label(
            14,
            146,
            222,
            8,
            "Where lines that do not register send the calls they dial.",
        )
        .label(14, 162, 34, 8, "&Host:")
        .edit(ID_OUTBOUND, 50, 160, 132, 14)
        .ok_cancel()
}

/// What the dialog changes of `[sip]`: only what is not as the service
/// has it.
pub fn change(before: &SipSettings, peers: &[String], outbound: &str) -> Option<SipChange> {
    let outbound = outbound.trim();
    let change = SipChange {
        trusted_peers: (peers != before.trusted_peers.as_slice()).then(|| peers.to_vec()),
        outbound_peer: (outbound != before.outbound_peer).then(|| outbound.to_string()),
        ..SipChange::default()
    };
    (change != SipChange::default()).then_some(change)
}

/// The peers with one more: an address that is there already is not
/// added again, and an empty one is message `RMD4009`.
pub fn with_peer(peers: &[String], address: &str) -> Result<Vec<String>, ErrorInfo> {
    let address = address.trim();
    if address.is_empty() {
        return Err(must_not_be_empty("Address"));
    }
    let mut peers = peers.to_vec();
    if !peers.iter().any(|peer| peer == address) {
        peers.push(address.to_string());
    }
    Ok(peers)
}

struct Peers {
    manager: Rc<Manager>,
    settings: Settings,
    peers: RefCell<Vec<String>>,
}

impl Peers {
    fn show(&self, dialog: Hwnd, selected: Option<usize>) {
        let list = ListBox::of(dialog.item(ID_PEERS));
        list.set_entries(&self.peers.borrow(), selected);
        dialog
            .item(ID_REMOVE)
            .set_enabled(list.selected().is_some());
    }

    fn add(&self, dialog: Hwnd) {
        let field = dialog.item(ID_PEER);
        let added = with_peer(&self.peers.borrow(), &field.text());
        match added {
            Ok(peers) => {
                let last = peers.len().checked_sub(1);
                *self.peers.borrow_mut() = peers;
                field.set_text("");
                self.show(dialog, last);
            }
            Err(error) => self.manager.show_error(dialog, &error),
        }
    }

    fn remove(&self, dialog: Hwnd) {
        let Some(index) = ListBox::of(dialog.item(ID_PEERS)).selected() else {
            return;
        };
        let left = {
            let mut peers = self.peers.borrow_mut();
            if index < peers.len() {
                peers.remove(index);
            }
            peers.len()
        };
        // The entry that took its place, or the last one.
        self.show(dialog, left.checked_sub(1).map(|last| index.min(last)));
    }

    /// Sends what was changed. `Ok` when there was nothing to send or the
    /// service took it.
    fn send(&self, dialog: Hwnd) -> Result<(), ErrorInfo> {
        let change = change(
            &self.settings.sip,
            &self.peers.borrow(),
            &dialog.item(ID_OUTBOUND).text(),
        );
        let Some(sip) = change else {
            return Ok(());
        };
        let call = Call::ConfigSet(SettingsChange {
            sip: Some(sip),
            ..SettingsChange::default()
        });
        self.manager
            .call::<SettingsResult>(dialog, call)
            .map(|_| ())
    }
}

impl Handler for Peers {
    fn event(&self, dialog: Hwnd, event: Event<'_>) -> Reply {
        match event {
            Event::InitDialog => {
                dialog
                    .item(ID_OUTBOUND)
                    .set_text(&self.settings.sip.outbound_peer);
                self.show(dialog, None);
                watch(&self.manager, dialog);
                Reply::Default
            }
            Event::Timer { id } if watched(&self.manager, dialog, id, "trunk-peers") => {
                Reply::Handled
            }
            Event::Command {
                id: ID_PEERS,
                code: code::LIST_SELECTED,
            } => {
                dialog.item(ID_REMOVE).set_enabled(true);
                Reply::Handled
            }
            Event::Command { id: ID_ADD, .. } => {
                self.add(dialog);
                Reply::Handled
            }
            Event::Command { id: ID_REMOVE, .. } => {
                self.remove(dialog);
                Reply::Handled
            }
            Event::Command { id: id::OK, .. } => {
                match self.send(dialog) {
                    Ok(()) => dialog.end_dialog(id::OK),
                    // The dialog stays, with what was typed.
                    Err(error) => self.manager.show_error(dialog, &error),
                }
                Reply::Handled
            }
            _ => Reply::Default,
        }
    }
}

/// Shows the trunk peers and sends what the user changes.
pub fn show(manager: &Rc<Manager>, owner: Hwnd) {
    // Asked for before the dialog opens, so that it opens filled in.
    let settings = match manager.call::<Settings>(owner, Call::ConfigGet) {
        Ok(settings) => settings,
        Err(error) => {
            manager.show_error(owner, &error);
            return;
        }
    };
    let handler = Rc::new(Peers {
        manager: Rc::clone(manager),
        peers: RefCell::new(settings.sip.trusted_peers.clone()),
        settings,
    });
    template().show_modal(Some(owner), handler);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sip() -> SipSettings {
        SipSettings {
            enabled: true,
            listen: "0.0.0.0:5060".to_string(),
            trusted_peers: vec!["10.10.20.11".to_string()],
            outbound_peer: "10.10.20.11".to_string(),
            ring_timeout_s: 60,
            ..SipSettings::default()
        }
    }

    #[test]
    fn nothing_touched_changes_nothing() {
        let before = sip();
        assert_eq!(
            change(&before, &before.trusted_peers, " 10.10.20.11 "),
            None
        );
    }

    #[test]
    fn only_what_was_changed_is_sent() {
        let before = sip();
        let peers = vec!["10.10.20.11".to_string(), "10.10.30.0/24".to_string()];
        let sent = change(&before, &peers, "10.10.20.11").unwrap();
        assert_eq!(sent.trusted_peers, Some(peers));
        assert_eq!(sent.outbound_peer, None);
        assert_eq!(sent.enabled, None);

        let sent = change(&before, &before.trusted_peers, "").unwrap();
        assert_eq!(sent.trusted_peers, None);
        assert_eq!(sent.outbound_peer.as_deref(), Some(""));
        // Every peer taken away is a change too.
        let sent = change(&before, &[], "10.10.20.11").unwrap();
        assert_eq!(sent.trusted_peers, Some(Vec::new()));
    }

    #[test]
    fn a_peer_is_added_once_and_never_empty() {
        let peers = vec!["10.10.20.11".to_string()];
        assert_eq!(
            with_peer(&peers, " 10.10.30.0/24 ").unwrap(),
            ["10.10.20.11", "10.10.30.0/24"]
        );
        assert_eq!(with_peer(&peers, "10.10.20.11").unwrap(), peers);
        let error = with_peer(&peers, "  ").unwrap_err();
        assert_eq!(error.msgid, "RMD4009");
        assert_eq!(error.text, "Address must not be empty.");
    }
}
