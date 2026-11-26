// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Menu bars, popup menus and accelerators.
//!
//! A menu is described as a tree of [`MenuItem`]s and built in one go. A
//! mnemonic is an ampersand before its letter, and an accelerator is
//! written after a tab, as in `&Reload configuration\tCtrl+R`; the keys
//! themselves are an [`Accelerators`] table, which the message loop reads.

use std::cell::OnceCell;

use windows::Win32::Foundation::POINT;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
    DeleteDC, HBITMAP, SelectObject,
};
use windows::Win32::UI::Shell::{
    SHGSI_ICON, SHGSI_SMALLICON, SHGetStockIconInfo, SHSTOCKICONINFO, SIID_SHIELD,
};
use windows::Win32::UI::WindowsAndMessaging::{
    ACCEL, ACCEL_VIRT_FLAGS, AppendMenuW, CheckMenuItem, CheckMenuRadioItem,
    CreateAcceleratorTableW, CreateMenu, CreatePopupMenu, DI_NORMAL, DestroyAcceleratorTable,
    DestroyIcon, DestroyMenu, DrawIconEx, DrawMenuBar, EnableMenuItem, FCONTROL, FVIRTKEY,
    GetCursorPos, GetMenu, GetMenuItemCount, GetMenuItemID, GetMenuState, GetMenuStringW,
    GetSubMenu, GetSystemMetrics, HACCEL, HMENU, MENUITEMINFOW, MF_BYCOMMAND, MF_BYPOSITION,
    MF_CHECKED, MF_DISABLED, MF_ENABLED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING,
    MF_UNCHECKED, MIIM_BITMAP, SM_CXSMICON, SetMenuItemInfoW, TPM_LEFTALIGN, TPM_RIGHTBUTTON,
    TrackPopupMenu,
};
use windows::core::PCWSTR;

use crate::util::{from_wide, pcwstr, wide};
use crate::window::Hwnd;

/// One entry of a menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuItem {
    /// A command: its id and its text.
    Command(u16, String),
    /// A line between groups of commands.
    Separator,
    /// A menu inside the menu: its text and its entries.
    Submenu(String, Vec<MenuItem>),
}

impl MenuItem {
    /// A command.
    pub fn command(id: u16, text: &str) -> MenuItem {
        MenuItem::Command(id, text.to_string())
    }

    /// A menu inside the menu.
    pub fn submenu(text: &str, items: Vec<MenuItem>) -> MenuItem {
        MenuItem::Submenu(text.to_string(), items)
    }
}

/// A menu: a menu bar or a popup menu.
///
/// A menu that has been made and not given to a window is destroyed when
/// dropped. A window's menu bar, from [`Menu::of_window`], belongs to the
/// window.
pub struct Menu {
    handle: HMENU,
    owned: bool,
}

fn fill(menu: HMENU, items: &[MenuItem]) {
    for item in items {
        // SAFETY: `menu` is a menu being built, each text is terminated
        // UTF-16 that outlives its call, and a submenu handle is given to
        // the menu, which destroys it with itself.
        unsafe {
            let _ = match item {
                MenuItem::Command(id, text) => {
                    let text = wide(text);
                    AppendMenuW(menu, MF_STRING, usize::from(*id), pcwstr(&text))
                }
                MenuItem::Separator => AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()),
                MenuItem::Submenu(text, items) => {
                    let Ok(submenu) = CreatePopupMenu() else {
                        continue;
                    };
                    fill(submenu, items);
                    let text = wide(text);
                    AppendMenuW(menu, MF_POPUP, submenu.0 as usize, pcwstr(&text))
                }
            };
        }
    }
}

impl Menu {
    /// A menu bar.
    pub fn bar(items: &[MenuItem]) -> Menu {
        // SAFETY: the call takes nothing. A null menu, which would mean
        // the system is out of resources, is harmless to fill and destroy.
        let handle = unsafe { CreateMenu() }.unwrap_or_default();
        fill(handle, items);
        Menu {
            handle,
            owned: true,
        }
    }

    /// A popup menu, for a context menu.
    pub fn popup(items: &[MenuItem]) -> Menu {
        // SAFETY: as in `bar`.
        let handle = unsafe { CreatePopupMenu() }.unwrap_or_default();
        fill(handle, items);
        Menu {
            handle,
            owned: true,
        }
    }

    /// The menu bar of a window, if it has one.
    pub fn of_window(window: Hwnd) -> Option<Menu> {
        // SAFETY: the call takes any handle.
        let handle = unsafe { GetMenu(window.0) };
        (!handle.is_invalid()).then_some(Menu {
            handle,
            owned: false,
        })
    }

    /// Gives up the menu to whoever takes the handle.
    pub(crate) fn into_handle(mut self) -> HMENU {
        self.owned = false;
        self.handle
    }

    /// Enables or greys a command, wherever in the menu it is.
    pub fn set_enabled(&self, id: u16, enabled: bool) {
        let flags = MF_BYCOMMAND | if enabled { MF_ENABLED } else { MF_GRAYED };
        // SAFETY: the call takes any menu handle and id.
        unsafe {
            let _ = EnableMenuItem(self.handle, u32::from(id), flags);
        }
    }

    /// Whether a command is enabled. A command the menu does not have is
    /// not.
    pub fn enabled(&self, id: u16) -> bool {
        // SAFETY: the call takes any menu handle and id.
        let state = unsafe { GetMenuState(self.handle, u32::from(id), MF_BYCOMMAND) };
        state != u32::MAX && state & (MF_GRAYED.0 | MF_DISABLED.0) == 0
    }

    /// Puts a check mark beside a command, or takes it away.
    pub fn set_checked(&self, id: u16, checked: bool) {
        let flags = MF_BYCOMMAND | if checked { MF_CHECKED } else { MF_UNCHECKED };
        // SAFETY: the call takes any menu handle and id.
        unsafe {
            CheckMenuItem(self.handle, u32::from(id), flags.0);
        }
    }

    /// Whether a command has a check mark.
    pub fn checked(&self, id: u16) -> bool {
        // SAFETY: the call takes any menu handle and id.
        let state = unsafe { GetMenuState(self.handle, u32::from(id), MF_BYCOMMAND) };
        state != u32::MAX && state & MF_CHECKED.0 != 0
    }

    /// Marks one command of the run `first` to `last` with a radio mark,
    /// and clears the others.
    pub fn set_radio(&self, first: u16, last: u16, chosen: u16) {
        // The commands may be in a submenu, which is where the call wants
        // to be made.
        for menu in self.menus() {
            // SAFETY: the call takes any menu handle and ids; in a menu
            // that does not hold the commands it fails.
            unsafe {
                let _ = CheckMenuRadioItem(
                    menu,
                    u32::from(first),
                    u32::from(last),
                    u32::from(chosen),
                    MF_BYCOMMAND.0,
                );
            }
        }
    }

    /// Puts the shield beside a command: choosing it will ask for
    /// elevation.
    pub fn set_shield(&self, id: u16) {
        let Some(bitmap) = shield_bitmap() else {
            return;
        };
        let info = MENUITEMINFOW {
            cbSize: size_of::<MENUITEMINFOW>() as u32,
            fMask: MIIM_BITMAP,
            hbmpItem: bitmap,
            ..MENUITEMINFOW::default()
        };
        // SAFETY: `info` is a whole MENUITEMINFOW with its size filled in,
        // and the bitmap lives as long as the thread does.
        unsafe {
            let _ = SetMenuItemInfoW(self.handle, u32::from(id), false, &info);
        }
    }

    /// Shows the menu as a context menu at a place on the screen, or at
    /// the pointer. The command chosen arrives at `owner` as an
    /// [`crate::Event::Command`].
    pub fn show_popup(&self, owner: Hwnd, position: Option<(i32, i32)>) {
        let (x, y) = position.unwrap_or_else(|| {
            let mut point = POINT::default();
            // SAFETY: `point` is valid for the call to fill.
            unsafe {
                let _ = GetCursorPos(&mut point);
            }
            (point.x, point.y)
        });
        owner.bring_to_front();
        // SAFETY: the menu and the owner are valid for the length of the
        // call, which returns when the menu closes.
        unsafe {
            let _ = TrackPopupMenu(
                self.handle,
                TPM_LEFTALIGN | TPM_RIGHTBUTTON,
                x,
                y,
                None,
                owner.0,
                None,
            );
        }
    }

    /// Has a window draw its menu bar again, after the bar itself (not a
    /// menu under it) was changed.
    pub fn redraw_bar(window: Hwnd) {
        // SAFETY: the call takes any handle.
        unsafe {
            let _ = DrawMenuBar(window.0);
        }
    }

    /// This menu and every menu under it.
    fn menus(&self) -> Vec<HMENU> {
        fn collect(menu: HMENU, out: &mut Vec<HMENU>) {
            out.push(menu);
            // SAFETY: the calls take any menu handle and position.
            let count = unsafe { GetMenuItemCount(Some(menu)) }.max(0);
            for position in 0..count {
                // SAFETY: as above.
                let submenu = unsafe { GetSubMenu(menu, position) };
                if !submenu.is_invalid() {
                    collect(submenu, out);
                }
            }
        }
        let mut out = Vec::new();
        collect(self.handle, &mut out);
        out
    }

    /// Every command of the menu and the menus under it: its id, its text
    /// with the mnemonic's ampersand and any accelerator, and the text of
    /// the menu it is in. For checking a whole menu tree.
    pub fn commands(&self) -> Vec<(u16, String, String)> {
        fn text_at(menu: HMENU, position: i32) -> String {
            let mut buffer = [0u16; 256];
            // SAFETY: the buffer is as long as the slice says.
            let length =
                unsafe { GetMenuStringW(menu, position as u32, Some(&mut buffer), MF_BYPOSITION) };
            from_wide(&buffer[..(length.max(0) as usize).min(buffer.len())])
        }
        fn collect(menu: HMENU, title: &str, out: &mut Vec<(u16, String, String)>) {
            // SAFETY: the calls take any menu handle and position.
            let count = unsafe { GetMenuItemCount(Some(menu)) }.max(0);
            for position in 0..count {
                let text = text_at(menu, position);
                // SAFETY: as above.
                let submenu = unsafe { GetSubMenu(menu, position) };
                if !submenu.is_invalid() {
                    collect(submenu, &text, out);
                    continue;
                }
                // SAFETY: as above. A separator has the id 0.
                let id = unsafe { GetMenuItemID(menu, position) };
                if id != 0 && id != u32::MAX {
                    out.push((id as u16, text, title.to_string()));
                }
            }
        }
        let mut out = Vec::new();
        collect(self.handle, "", &mut out);
        out
    }
}

impl Drop for Menu {
    fn drop(&mut self) {
        if self.owned {
            // SAFETY: the menu was made here and given to nobody.
            unsafe {
                let _ = DestroyMenu(self.handle);
            }
        }
    }
}

thread_local! {
    /// The shield as a bitmap for menus, made once for the thread.
    static SHIELD: OnceCell<Option<HBITMAP>> = const { OnceCell::new() };
}

/// The system's shield icon as a 32-bit bitmap the size of a small icon.
fn shield_bitmap() -> Option<HBITMAP> {
    SHIELD.with(|cell| {
        *cell.get_or_init(|| {
            let mut info = SHSTOCKICONINFO {
                cbSize: size_of::<SHSTOCKICONINFO>() as u32,
                ..SHSTOCKICONINFO::default()
            };
            // SAFETY: `info` has its size filled in and is valid for the
            // call to fill; the icon it returns is destroyed below.
            unsafe { SHGetStockIconInfo(SIID_SHIELD, SHGSI_ICON | SHGSI_SMALLICON, &mut info) }
                .ok()?;
            // SAFETY: the call takes an index and returns a number.
            let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16);
            let header = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: size,
                    biHeight: -size,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..BITMAPINFOHEADER::default()
                },
                ..BITMAPINFO::default()
            };
            // SAFETY: a memory device context and a zeroed 32-bit bitmap
            // are made, the icon is drawn into the bitmap with its alpha,
            // and the context and the icon are released. The bitmap is
            // kept for the menus for the life of the thread.
            unsafe {
                let dc = CreateCompatibleDC(None);
                let mut bits = std::ptr::null_mut();
                let bitmap =
                    CreateDIBSection(Some(dc), &header, DIB_RGB_COLORS, &mut bits, None, 0);
                let drawn = bitmap.as_ref().ok().map(|bitmap| {
                    let previous = SelectObject(dc, (*bitmap).into());
                    let _ = DrawIconEx(dc, 0, 0, info.hIcon, size, size, 0, None, DI_NORMAL);
                    SelectObject(dc, previous);
                    *bitmap
                });
                let _ = DeleteDC(dc);
                let _ = DestroyIcon(info.hIcon);
                drawn
            }
        })
    })
}

/// A key of an accelerator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// Ctrl and a letter.
    Ctrl(char),
    /// A function key, 1 to 24.
    Function(u8),
    /// Delete.
    Delete,
}

/// A table of accelerators: keys that send commands to the main window.
pub struct Accelerators {
    handle: HACCEL,
}

impl Accelerators {
    /// A table from keys and the commands they send.
    pub fn new(keys: &[(Key, u16)]) -> Accelerators {
        /// The virtual key codes of F1 and Delete; letters are their own
        /// upper-case ASCII.
        const VK_F1: u16 = 0x70;
        const VK_DELETE: u16 = 0x2E;
        let table: Vec<ACCEL> = keys
            .iter()
            .map(|(key, command)| {
                let (flags, code) = match key {
                    Key::Ctrl(letter) => (FVIRTKEY | FCONTROL, letter.to_ascii_uppercase() as u16),
                    Key::Function(number) => {
                        (FVIRTKEY, VK_F1 + u16::from(number.clamp(&1, &24) - 1))
                    }
                    Key::Delete => (FVIRTKEY, VK_DELETE),
                };
                ACCEL {
                    fVirt: ACCEL_VIRT_FLAGS(flags.0),
                    key: code,
                    cmd: *command,
                }
            })
            .collect();
        // SAFETY: the table is a slice of whole ACCEL structures. A null
        // table, from an empty slice or a system out of resources,
        // translates nothing and is harmless to destroy.
        let handle = unsafe { CreateAcceleratorTableW(&table) }.unwrap_or_default();
        Accelerators { handle }
    }

    pub(crate) fn handle(&self) -> HACCEL {
        self.handle
    }
}

impl Drop for Accelerators {
    fn drop(&mut self) {
        // SAFETY: the table was made by `new` and is destroyed nowhere
        // else.
        unsafe {
            let _ = DestroyAcceleratorTable(self.handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Menu {
        Menu::bar(&[
            MenuItem::submenu(
                "&File",
                vec![
                    MenuItem::command(10, "&Reload\tCtrl+R"),
                    MenuItem::Separator,
                    MenuItem::command(11, "E&xit\tAlt+F4"),
                ],
            ),
            MenuItem::submenu(
                "&View",
                vec![
                    MenuItem::submenu(
                        "&Show",
                        vec![
                            MenuItem::command(20, "&All lines"),
                            MenuItem::command(21, "&Idle"),
                            MenuItem::command(22, "&Disabled"),
                        ],
                    ),
                    MenuItem::command(23, "&Toolbar"),
                ],
            ),
        ])
    }

    #[test]
    fn a_menu_is_built_as_described() {
        let menu = sample();
        assert_eq!(
            menu.commands(),
            [
                (10, "&Reload\tCtrl+R".to_string(), "&File".to_string()),
                (11, "E&xit\tAlt+F4".to_string(), "&File".to_string()),
                (20, "&All lines".to_string(), "&Show".to_string()),
                (21, "&Idle".to_string(), "&Show".to_string()),
                (22, "&Disabled".to_string(), "&Show".to_string()),
                (23, "&Toolbar".to_string(), "&View".to_string()),
            ]
        );
        assert_eq!(Menu::popup(&[]).commands(), []);
    }

    #[test]
    fn commands_are_enabled_checked_and_marked_wherever_they_are() {
        let menu = sample();
        assert!(menu.enabled(10) && menu.enabled(21));
        menu.set_enabled(10, false);
        menu.set_enabled(21, false);
        assert!(!menu.enabled(10) && !menu.enabled(21));
        assert!(menu.enabled(11));
        menu.set_enabled(21, true);
        assert!(menu.enabled(21));
        // A command the menu does not have is not enabled.
        assert!(!menu.enabled(99));

        assert!(!menu.checked(23));
        menu.set_checked(23, true);
        assert!(menu.checked(23));
        menu.set_checked(23, false);
        assert!(!menu.checked(23));

        // The radio mark is on one command of a run in a submenu.
        menu.set_radio(20, 22, 21);
        assert!(menu.checked(21) && !menu.checked(20) && !menu.checked(22));
        menu.set_radio(20, 22, 22);
        assert!(menu.checked(22) && !menu.checked(21));
        // The shield does not disturb the command.
        menu.set_shield(10);
        assert_eq!(menu.commands()[0].1, "&Reload\tCtrl+R");
    }

    #[test]
    fn an_accelerator_table_is_made_from_keys() {
        let table = Accelerators::new(&[
            (Key::Ctrl('r'), 10),
            (Key::Function(5), 11),
            (Key::Delete, 12),
        ]);
        assert!(!table.handle().is_invalid());
        // An empty table translates nothing and can still be dropped.
        drop(Accelerators::new(&[]));
    }
}
