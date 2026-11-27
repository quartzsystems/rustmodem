// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! A control the program paints: a static control that is drawn by its
//! owner, and the [`Canvas`] it is drawn on.
//!
//! There is one use for it, the scope of the line diagnostics, for which
//! no stock control exists. Everything is painted on a bitmap in memory
//! and copied to the screen in one piece when the canvas is dropped, so
//! that a picture that changes several times a second does not flicker.

use windows::Win32::Foundation::{COLORREF, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    BitBlt, COLOR_GRAYTEXT, COLOR_WINDOW, COLOR_WINDOWTEXT, CreateCompatibleBitmap,
    CreateCompatibleDC, CreatePen, CreateSolidBrush, DeleteDC, DeleteObject, FillRect, GetSysColor,
    HBITMAP, HDC, HGDIOBJ, PS_SOLID, Polyline, SRCCOPY, SYS_COLOR_INDEX, SelectObject,
};
use windows::Win32::UI::Controls::DRAWITEMSTRUCT;

use crate::window::Colour;

impl Colour {
    fn system(index: SYS_COLOR_INDEX) -> Colour {
        // SAFETY: the call takes a plain number and returns one.
        Colour::from_colorref(unsafe { GetSysColor(index) })
    }

    /// The background of a window that shows something to read: white,
    /// unless the user has it otherwise.
    pub fn window() -> Colour {
        Colour::system(COLOR_WINDOW)
    }

    /// The colour of text in a window.
    pub fn window_text() -> Colour {
        Colour::system(COLOR_WINDOWTEXT)
    }

    /// The colour of text that is greyed.
    pub fn gray_text() -> Colour {
        Colour::system(COLOR_GRAYTEXT)
    }
}

/// What a control that is drawn by its owner is painted on, in pixels
/// from its top left corner. What is painted shows when the canvas is
/// dropped.
#[derive(Debug)]
pub struct Canvas {
    target: HDC,
    area: RECT,
    memory: HDC,
    bitmap: HBITMAP,
    /// What the memory context had selected before the bitmap.
    before: HGDIOBJ,
}

impl Canvas {
    /// The canvas of a `WM_DRAWITEM`: `None` if no bitmap could be made,
    /// in which case nothing is painted.
    pub(crate) fn of(item: &DRAWITEMSTRUCT) -> Option<Canvas> {
        let area = item.rcItem;
        let (width, height) = (area.right - area.left, area.bottom - area.top);
        if width <= 0 || height <= 0 {
            return None;
        }
        // SAFETY: `item.hDC` is the device context Windows gave for the
        // control, valid for the message. What is made here is released
        // in `drop`, or here if making the rest of it fails.
        unsafe {
            let memory = CreateCompatibleDC(Some(item.hDC));
            if memory.is_invalid() {
                return None;
            }
            let bitmap = CreateCompatibleBitmap(item.hDC, width, height);
            if bitmap.is_invalid() {
                let _ = DeleteDC(memory);
                return None;
            }
            let before = SelectObject(memory, bitmap.into());
            Some(Canvas {
                target: item.hDC,
                area,
                memory,
                bitmap,
                before,
            })
        }
    }

    /// The width in pixels.
    pub fn width(&self) -> i32 {
        self.area.right - self.area.left
    }

    /// The height in pixels.
    pub fn height(&self) -> i32 {
        self.area.bottom - self.area.top
    }

    /// Fills the whole canvas with a colour.
    pub fn fill(&mut self, colour: Colour) {
        let whole = RECT {
            left: 0,
            top: 0,
            right: self.width(),
            bottom: self.height(),
        };
        // SAFETY: the memory context is this canvas's own; the brush is
        // made, used and deleted here.
        unsafe {
            let brush = CreateSolidBrush(colour.colorref());
            FillRect(self.memory, &whole, brush);
            let _ = DeleteObject(brush.into());
        }
    }

    /// Draws a line one pixel wide through the points, in order.
    pub fn polyline(&mut self, points: &[(i32, i32)], colour: Colour) {
        if points.len() < 2 {
            return;
        }
        let points: Vec<POINT> = points.iter().map(|(x, y)| POINT { x: *x, y: *y }).collect();
        let colour: COLORREF = colour.colorref();
        // SAFETY: the memory context is this canvas's own; the pen is
        // made, selected, taken out again and deleted here.
        unsafe {
            let pen = CreatePen(PS_SOLID, 1, colour);
            let before = SelectObject(self.memory, pen.into());
            let _ = Polyline(self.memory, &points);
            SelectObject(self.memory, before);
            let _ = DeleteObject(pen.into());
        }
    }

    /// Draws a line one pixel wide from one point to another.
    pub fn line(&mut self, from: (i32, i32), to: (i32, i32), colour: Colour) {
        self.polyline(&[from, to], colour);
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        // SAFETY: the contexts and the bitmap are the ones `of` was given
        // and made, and are released once, here.
        unsafe {
            let _ = BitBlt(
                self.target,
                self.area.left,
                self.area.top,
                self.width(),
                self.height(),
                Some(self.memory),
                0,
                0,
                SRCCOPY,
            );
            SelectObject(self.memory, self.before);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.memory);
        }
    }
}
