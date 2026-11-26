// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Rendering a window to pixels, for the screenshot tests.
//!
//! A picture of a window is put together from two sources, because
//! neither gives all of it:
//!
//! - What is inside the frame (the client area, with every control) is
//!   copied from the window's own surface: what the window has drawn, as
//!   it stands. This is exact and the same every time, and it does not
//!   need the window to be in front, since the desktop compositor keeps a
//!   surface for every window.
//! - The frame and what is in it (the title bar, the menu bar) is drawn
//!   by the compositor and by the system, and is not on that surface. For
//!   it the window is rendered through `PrintWindow` with the flag that
//!   has the compositor give what it shows. That rendering has the window
//!   draw all of itself again and does not wait for it to finish, so a
//!   window with many controls comes out with some of them missing: it is
//!   used for the frame only.
//!
//! A window is larger than it looks: around the frame that is drawn there
//! is an invisible border for resizing, which comes out black. The
//! picture is cut down to the frame the compositor draws.

use windows::Win32::Foundation::{POINT, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, ClientToScreen, CreateCompatibleDC,
    CreateDIBSection, DCX_CACHE, DCX_WINDOW, DIB_RGB_COLORS, DeleteDC, DeleteObject, GdiFlush,
    GetDCEx, HDC, ReleaseDC, SRCCOPY, SelectObject,
};
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows::Win32::System::Threading::{
    GR_GDIOBJECTS, GR_USEROBJECTS, GetCurrentProcess, GetGuiResources,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetCursorPos, GetWindowRect, PW_RENDERFULLCONTENT, SetCursorPos,
};

use crate::window::{Hwnd, Rect};

/// A picture: `width * height * 4` bytes of red, green, blue and alpha,
/// top row first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    /// The width in pixels.
    pub width: u32,
    /// The height in pixels.
    pub height: u32,
    /// The pixels.
    pub rgba: Vec<u8>,
}

/// Renders a window, frame and all, to pixels. `None` if the handle is
/// not a window, the window has no size, or it could not be drawn.
///
/// The client area is what the window has drawn so far: a caller that
/// wants a window that has just changed lets it handle its messages
/// first.
pub fn capture(window: Hwnd) -> Option<Image> {
    let mut rect = RECT::default();
    // SAFETY: `rect` is valid for the call to fill.
    unsafe { GetWindowRect(window.0, &mut rect) }.ok()?;
    let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
    if width <= 0 || height <= 0 {
        return None;
    }
    // The client area, within the window's rectangle.
    let mut client = RECT::default();
    let mut origin = POINT::default();
    // SAFETY: both structures are valid for the calls to fill.
    unsafe {
        GetClientRect(window.0, &mut client).ok()?;
        let _ = ClientToScreen(window.0, &mut origin);
    }
    let inside = Rect::new(
        origin.x - rect.left,
        origin.y - rect.top,
        client.right - client.left,
        client.bottom - client.top,
    );

    // The surface first: rendering the frame has the window draw again.
    let surface = draw(width, height, |dc| {
        // SAFETY: a device context for the whole window, not clipped to
        // exclude its controls, is taken, copied from and released. `dc`
        // is the memory device context `draw` made.
        unsafe {
            let source = GetDCEx(Some(window.0), None, DCX_WINDOW | DCX_CACHE);
            if source.is_invalid() {
                return false;
            }
            let copied = BitBlt(dc, 0, 0, width, height, Some(source), 0, 0, SRCCOPY).is_ok();
            ReleaseDC(Some(window.0), source);
            copied
        }
    })?;
    let mut whole = draw(width, height, |dc| {
        // SAFETY: the window draws into the memory device context `draw`
        // made; a handle that is no longer a window draws nothing.
        unsafe { PrintWindow(window.0, dc, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT)).as_bool() }
    })?;
    whole.paste(&surface, inside);

    // The frame that is drawn, within the window's rectangle.
    let mut frame = RECT::default();
    // SAFETY: `frame` is a RECT, which is what this attribute is, and the
    // size given is its size.
    let asked = unsafe {
        DwmGetWindowAttribute(
            window.0,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&raw mut frame).cast(),
            size_of::<RECT>() as u32,
        )
    };
    if asked.is_err() {
        return Some(whole);
    }
    Some(whole.cropped(
        frame.left - rect.left,
        frame.top - rect.top,
        frame.right - frame.left,
        frame.bottom - frame.top,
    ))
}

/// Makes a bitmap of a size, has `paint` draw into it, and returns its
/// pixels as a picture. `None` if the bitmap could not be made or `paint`
/// says that it drew nothing.
fn draw(width: i32, height: i32, paint: impl FnOnce(HDC) -> bool) -> Option<Image> {
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            // A negative height is a bitmap whose first row is the top.
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..BITMAPINFOHEADER::default()
        },
        ..BITMAPINFO::default()
    };
    let count = width as usize * height as usize * 4;
    // SAFETY: a memory device context and a 32-bit bitmap are made,
    // `paint` draws into them, and both are released before this returns.
    // `bits` is the address of the bitmap's `count` bytes, which are read
    // only while the bitmap exists and after GDI has finished drawing.
    unsafe {
        let dc = CreateCompatibleDC(None);
        if dc.is_invalid() {
            return None;
        }
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let Ok(bitmap) = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)
        else {
            let _ = DeleteDC(dc);
            return None;
        };
        let previous = SelectObject(dc, bitmap.into());
        let drawn = paint(dc);
        let _ = GdiFlush();
        let rgba = (drawn && !bits.is_null()).then(|| {
            let pixels = std::slice::from_raw_parts(bits.cast::<u8>(), count);
            let mut rgba = Vec::with_capacity(count);
            for pixel in pixels.as_chunks::<4>().0 {
                // Blue, green, red in the bitmap. The alpha is whatever
                // was left there; the picture is opaque.
                rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
            }
            rgba
        });
        SelectObject(dc, previous);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(dc);
        Some(Image {
            width: width as u32,
            height: height as u32,
            rgba: rgba?,
        })
    }
}

/// How many GDI objects and how many USER objects this process holds. A
/// program that leaks either holds more and more of them.
pub fn gui_handles() -> (u32, u32) {
    // SAFETY: the calls take the pseudo-handle of this process, which
    // needs no closing, and an index, and return a count.
    unsafe {
        let process = GetCurrentProcess();
        (
            GetGuiResources(process, GR_GDIOBJECTS),
            GetGuiResources(process, GR_USEROBJECTS),
        )
    }
}

/// Where the pointer is, in screen coordinates.
pub fn pointer() -> (i32, i32) {
    let mut point = POINT::default();
    // SAFETY: `point` is valid for the call to fill.
    unsafe {
        let _ = GetCursorPos(&mut point);
    }
    (point.x, point.y)
}

/// Puts the pointer somewhere. A window under the pointer shows it: a
/// row lights up, a button glows. A test that takes pictures first puts
/// the pointer where there is no window of its own.
pub fn move_pointer(x: i32, y: i32) {
    // SAFETY: the call takes two numbers.
    unsafe {
        let _ = SetCursorPos(x, y);
    }
}

impl Image {
    /// Copies the part of another picture of the same size that is inside
    /// a rectangle over the same part of this one. What of the rectangle
    /// is outside the pictures is left out.
    fn paste(&mut self, other: &Image, part: Rect) {
        if (other.width, other.height) != (self.width, self.height) {
            return;
        }
        let left = part.x.clamp(0, self.width as i32) as usize;
        let right = (part.x + part.width).clamp(0, self.width as i32) as usize;
        let top = part.y.clamp(0, self.height as i32) as usize;
        let bottom = (part.y + part.height).clamp(0, self.height as i32) as usize;
        if left >= right {
            return;
        }
        for row in top..bottom {
            let from = (row * self.width as usize + left) * 4;
            let to = (row * self.width as usize + right) * 4;
            self.rgba[from..to].copy_from_slice(&other.rgba[from..to]);
        }
    }

    /// The part of the picture inside a rectangle, which is cut to the
    /// picture first. A rectangle with nothing in the picture gives the
    /// whole picture.
    pub fn cropped(&self, x: i32, y: i32, width: i32, height: i32) -> Image {
        let left = x.clamp(0, self.width as i32);
        let top = y.clamp(0, self.height as i32);
        let right = (x + width).clamp(left, self.width as i32);
        let bottom = (y + height).clamp(top, self.height as i32);
        if right == left || bottom == top {
            return self.clone();
        }
        let (left, top) = (left as usize, top as usize);
        let (right, bottom) = (right as usize, bottom as usize);
        let stride = self.width as usize * 4;
        let mut rgba = Vec::with_capacity((right - left) * (bottom - top) * 4);
        for row in top..bottom {
            rgba.extend_from_slice(&self.rgba[row * stride + left * 4..row * stride + right * 4]);
        }
        Image {
            width: (right - left) as u32,
            height: (bottom - top) as u32,
            rgba,
        }
    }

    /// The pixel at a place: red, green, blue and alpha.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let at = (y as usize * self.width as usize + x as usize) * 4;
        [
            self.rgba[at],
            self.rgba[at + 1],
            self.rgba[at + 2],
            self.rgba[at + 3],
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture() -> Image {
        // Four by three, each pixel its own number.
        let rgba = (0..12u8).flat_map(|n| [n, n, n, 255]).collect();
        Image {
            width: 4,
            height: 3,
            rgba,
        }
    }

    #[test]
    fn a_picture_is_cut_to_a_rectangle() {
        let cut = picture().cropped(1, 1, 2, 2);
        assert_eq!((cut.width, cut.height), (2, 2));
        let numbers: Vec<u8> = cut.rgba.as_chunks::<4>().0.iter().map(|p| p[0]).collect();
        assert_eq!(numbers, [5, 6, 9, 10]);
        assert_eq!(cut.pixel(1, 1), [10, 10, 10, 255]);
        assert_eq!(picture().pixel(3, 2), [11, 11, 11, 255]);
    }

    #[test]
    fn a_rectangle_is_cut_to_the_picture_first() {
        let cut = picture().cropped(-5, 2, 7, 9);
        assert_eq!((cut.width, cut.height), (2, 1));
        assert_eq!(cut.pixel(0, 0)[0], 8);
        // Nothing of it in the picture: the whole picture.
        assert_eq!(picture().cropped(10, 10, 5, 5), picture());
        assert_eq!(picture().cropped(0, 0, 0, 0), picture());
        assert_eq!(picture().cropped(0, 0, 4, 3), picture());
    }

    #[test]
    fn a_handle_that_is_not_a_window_cannot_be_captured() {
        assert_eq!(capture(Hwnd::from_raw(0)), None);
    }
}
