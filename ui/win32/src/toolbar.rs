// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The toolbar: `ToolbarWindow32`, flat, with text labels beside the
//! images and tooltips.

use windows::Win32::Foundation::SIZE;
use windows::Win32::UI::Controls::{
    BTNS_AUTOSIZE, BTNS_BUTTON, BTNS_SEP, BTNS_SHOWTEXT, CCS_NODIVIDER, CCS_NOPARENTALIGN,
    CCS_NORESIZE, TB_ADDBUTTONSW, TB_AUTOSIZE, TB_BUTTONCOUNT, TB_BUTTONSTRUCTSIZE,
    TB_ENABLEBUTTON, TB_GETMAXSIZE, TB_ISBUTTONENABLED, TB_SETEXTENDEDSTYLE, TB_SETIMAGELIST,
    TBBUTTON, TBSTATE_ENABLED, TBSTYLE_EX_MIXEDBUTTONS, TBSTYLE_FLAT, TBSTYLE_LIST,
    TBSTYLE_TOOLTIPS, TOOLBARCLASSNAMEW,
};

use crate::imagelist::ImageList;
use crate::util::wide;
use crate::window::{Hwnd, Window};

/// One entry of a toolbar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolButton {
    /// A button: the command it sends, its label, and the index of its
    /// image in the toolbar's image list.
    Button {
        /// The command id.
        id: u16,
        /// The label, shown beside the image.
        text: String,
        /// The index of the image.
        image: usize,
    },
    /// A gap between groups of buttons.
    Separator,
}

impl ToolButton {
    /// A button.
    pub fn new(id: u16, text: &str, image: usize) -> ToolButton {
        ToolButton::Button {
            id,
            text: text.to_string(),
            image,
        }
    }
}

/// A toolbar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Toolbar(Hwnd);

impl Toolbar {
    /// Makes a toolbar in `parent`. It does not place itself: the window
    /// that owns it gives it its rectangle. The tip of each button is
    /// asked for with [`crate::Event::ToolTip`] when it is needed.
    pub fn create(parent: Hwnd, id: u16, images: &ImageList, buttons: &[ToolButton]) -> Toolbar {
        let style = TBSTYLE_FLAT
            | TBSTYLE_LIST
            | TBSTYLE_TOOLTIPS
            | (CCS_NODIVIDER | CCS_NORESIZE | CCS_NOPARENTALIGN) as u32;
        let bar = Window::control(parent, TOOLBARCLASSNAMEW, "", style, 0, id);
        // The toolbar has to be told the size of the structure before it
        // is given any buttons.
        bar.send(TB_BUTTONSTRUCTSIZE, size_of::<TBBUTTON>(), 0);
        // With this, only the buttons that ask for it show their text,
        // and the rest would show it as their tip; here every button asks.
        bar.send(TB_SETEXTENDEDSTYLE, 0, TBSTYLE_EX_MIXEDBUTTONS as isize);
        let toolbar = Toolbar(bar);
        toolbar.set_images(images);

        // The labels have to stay where they are until the buttons have
        // been added, when the toolbar has copied them.
        let labels: Vec<Vec<u16>> = buttons
            .iter()
            .map(|button| match button {
                ToolButton::Button { text, .. } => wide(text),
                ToolButton::Separator => wide(""),
            })
            .collect();
        let described: Vec<TBBUTTON> = buttons
            .iter()
            .zip(&labels)
            .map(|(button, label)| match button {
                ToolButton::Button { id, image, .. } => TBBUTTON {
                    iBitmap: *image as i32,
                    idCommand: i32::from(*id),
                    fsState: TBSTATE_ENABLED as u8,
                    fsStyle: (BTNS_BUTTON | BTNS_AUTOSIZE | BTNS_SHOWTEXT) as u8,
                    iString: label.as_ptr() as isize,
                    ..TBBUTTON::default()
                },
                ToolButton::Separator => TBBUTTON {
                    fsStyle: BTNS_SEP as u8,
                    ..TBBUTTON::default()
                },
            })
            .collect();
        // The array and the labels it points at are valid for the length
        // of the call, which copies them.
        bar.send(TB_ADDBUTTONSW, described.len(), described.as_ptr() as isize);
        bar.send(TB_AUTOSIZE, 0, 0);
        toolbar
    }

    /// The control.
    pub fn hwnd(self) -> Hwnd {
        self.0
    }

    /// Gives the toolbar its images, at the start and again when the DPI
    /// changes. The list must outlive the control's use of it.
    pub fn set_images(self, images: &ImageList) {
        self.0.send(TB_SETIMAGELIST, 0, images.handle().0);
        self.0.send(TB_AUTOSIZE, 0, 0);
    }

    /// Enables or disables a button.
    pub fn set_enabled(self, id: u16, enabled: bool) {
        if self.enabled(id) != enabled {
            self.0
                .send(TB_ENABLEBUTTON, usize::from(id), isize::from(enabled));
        }
    }

    /// Whether a button is enabled.
    pub fn enabled(self, id: u16) -> bool {
        self.0.send(TB_ISBUTTONENABLED, usize::from(id), 0) != 0
    }

    /// How many entries the toolbar has, separators included.
    pub fn count(self) -> usize {
        self.0.send(TB_BUTTONCOUNT, 0, 0).max(0) as usize
    }

    /// The width and height the buttons need, in pixels.
    pub fn size(self) -> (i32, i32) {
        let mut size = SIZE::default();
        // The structure is valid for the call to fill.
        self.0.send(TB_GETMAXSIZE, 0, (&raw mut size) as isize);
        (size.cx, size.cy)
    }
}
