// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! About rustmodem Manager.

use std::rc::Rc;

use win32::{DialogTemplate, Event, Handler, Hwnd, Reply, id, units};

use super::{watch, watched};
use crate::window::Manager;

const ID_ICON: u16 = 300;

/// The name of the product.
const PRODUCT: &str = "rustmodem Manager";

/// The copyright line.
const COPYRIGHT: &str = "Copyright (C) 2026 Quartz Systems";

/// What is said of the licence.
const LICENCE: &str = "This program is free software under the terms of the MIT license. It comes \
                       with no warranty of any kind.";

fn template() -> DialogTemplate {
    let (width, height) = (230, 104);
    DialogTemplate::new("About rustmodem Manager", width, height)
        .icon(ID_ICON, 7, 7)
        .label(36, 7, 187, 8, PRODUCT)
        .label(36, 19, 187, 8, &version_line())
        .label(36, 31, 187, 8, COPYRIGHT)
        .label(36, 47, 187, 26, LICENCE)
        .default_button(
            id::OK,
            width - units::MARGIN - units::BUTTON_WIDTH,
            height - units::MARGIN - units::BUTTON_HEIGHT,
            units::BUTTON_WIDTH,
            units::BUTTON_HEIGHT,
            "OK",
        )
}

/// The version of the manager, which is that of the product.
fn version_line() -> String {
    format!("Version {}", env!("CARGO_PKG_VERSION"))
}

struct About {
    manager: Rc<Manager>,
}

impl Handler for About {
    fn event(&self, dialog: Hwnd, event: Event<'_>) -> Reply {
        match event {
            Event::InitDialog => {
                dialog.item(ID_ICON).show_program_icon();
                watch(&self.manager, dialog);
                Reply::Default
            }
            Event::Timer { id } if watched(&self.manager, dialog, id, "about") => Reply::Handled,
            _ => Reply::Default,
        }
    }
}

/// Shows the product name, the version, the copyright and the licence.
pub fn show(manager: &Rc<Manager>, owner: Hwnd) {
    let handler = Rc::new(About {
        manager: Rc::clone(manager),
    });
    template().show_modal(Some(owner), handler);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dialog_says_what_the_product_is_and_whose() {
        assert_eq!(COPYRIGHT, "Copyright (C) 2026 Quartz Systems");
        assert!(LICENCE.contains("MIT"));
        let version = version_line();
        assert!(version.starts_with("Version "));
        assert_eq!(version.matches('.').count(), 2, "{version}");
        assert!(template().ids().contains(&ID_ICON));
    }
}
