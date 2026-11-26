// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Property sheets: the system's tabbed dialog with OK, Cancel and Apply.
//!
//! Each page is a [`DialogTemplate`] with a [`Handler`]. A page gets
//! [`crate::Event::InitDialog`] the first time it is shown, and
//! [`crate::Event::Apply`] when OK or Apply is pressed, if it has been
//! shown. A page that changes something tells the sheet with
//! [`Hwnd::page_changed`], which is what enables Apply.

use std::rc::Rc;

use windows::Win32::Foundation::LPARAM;
use windows::Win32::UI::Controls::{
    PROPSHEETHEADERW_V2, PROPSHEETPAGEW, PSH_NOCONTEXTHELP, PSH_PROPSHEETPAGE, PSM_CHANGED,
    PSP_DLGINDIRECT, PSP_USETITLE, PropertySheetW,
};
use windows::Win32::UI::WindowsAndMessaging::DLGTEMPLATE;

use crate::dialog::DialogTemplate;
use crate::util::{instance, pcwstr, wide};
use crate::window::{Handler, Hwnd};

/// Shows a property sheet and returns when it is closed. Each page is its
/// tab's title, its template and its handler. Returns whether the sheet
/// was closed with OK, or had something applied before it was cancelled.
pub fn property_sheet(
    owner: Option<Hwnd>,
    title: &str,
    pages: Vec<(String, DialogTemplate, Rc<dyn Handler>)>,
) -> bool {
    let title = wide(title);
    // Everything the page descriptions point at lives here until the
    // sheet has closed: the tab titles, the templates, and the handlers.
    // A page that is never looked at is never created, so the pages
    // cannot be what frees their handlers.
    let tabs: Vec<Vec<u16>> = pages.iter().map(|(tab, _, _)| wide(tab)).collect();
    let templates: Vec<Vec<u32>> = pages
        .iter()
        .map(|(_, template, _)| template.words(true))
        .collect();
    let handlers: Vec<Box<Rc<dyn Handler>>> = pages
        .into_iter()
        .map(|(_, _, handler)| Box::new(handler))
        .collect();

    let mut described: Vec<PROPSHEETPAGEW> = (0..handlers.len())
        .map(|index| {
            let mut page = PROPSHEETPAGEW {
                dwSize: size_of::<PROPSHEETPAGEW>() as u32,
                dwFlags: PSP_DLGINDIRECT | PSP_USETITLE,
                hInstance: instance(),
                pszTitle: pcwstr(&tabs[index]),
                pfnDlgProc: DialogTemplate::page_procedure(),
                lParam: LPARAM(&raw const *handlers[index] as isize),
                ..PROPSHEETPAGEW::default()
            };
            page.Anonymous1.pResource = templates[index].as_ptr() as *mut DLGTEMPLATE;
            page
        })
        .collect();

    let mut header = PROPSHEETHEADERW_V2 {
        dwSize: size_of::<PROPSHEETHEADERW_V2>() as u32,
        dwFlags: PSH_PROPSHEETPAGE | PSH_NOCONTEXTHELP,
        hwndParent: owner.map(|owner| owner.0).unwrap_or_default(),
        hInstance: instance(),
        pszCaption: pcwstr(&title),
        nPages: described.len() as u32,
        ..PROPSHEETHEADERW_V2::default()
    };
    header.Anonymous3.ppsp = described.as_mut_ptr();
    // SAFETY: the header and the pages are whole structures with their
    // sizes filled in; every pointer in them is to something above that
    // lives until after the call, which returns when the sheet has closed
    // and every page it created has been destroyed.
    let result = unsafe { PropertySheetW(&mut header) };
    drop(handlers);
    result > 0
}

impl Hwnd {
    /// For a page of a property sheet: tells the sheet that something on
    /// the page has changed, which enables Apply.
    pub fn page_changed(self) {
        if let Some(sheet) = self.parent() {
            sheet.send(PSM_CHANGED, self.raw() as usize, 0);
        }
    }
}
