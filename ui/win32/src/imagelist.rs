// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Image lists, and bitmaps made from pixels.
//!
//! The images of the manager are drawn in code and arrive here as red,
//! green, blue and alpha. Windows wants them as 32-bit bitmaps in blue,
//! green, red, alpha order with the colour multiplied by the alpha.

use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateDIBSection, DIB_RGB_COLORS, DeleteObject, HBITMAP,
};
use windows::Win32::UI::Controls::{
    HIMAGELIST, ILC_COLOR32, ImageList_Add, ImageList_Create, ImageList_Destroy,
    ImageList_GetImageCount,
};

/// A 32-bit bitmap from `width * height * 4` bytes of red, green, blue
/// and alpha, top row first. `None` if the pixels are not that many or
/// the bitmap cannot be made. The caller owns the bitmap.
pub(crate) fn bitmap_from_rgba(rgba: &[u8], width: i32, height: i32) -> Option<HBITMAP> {
    let (w, h) = (usize::try_from(width).ok()?, usize::try_from(height).ok()?);
    if w == 0 || h == 0 || rgba.len() != w * h * 4 {
        return None;
    }
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
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    // SAFETY: `info` describes a 32-bit bitmap and `bits` is where the
    // call writes the address of its pixels.
    let bitmap =
        unsafe { CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0) }.ok()?;
    if bits.is_null() {
        return None;
    }
    // SAFETY: the bitmap was just made with `w * h` pixels of four bytes
    // at `bits`, and nothing else has the bitmap yet.
    let pixels = unsafe { std::slice::from_raw_parts_mut(bits.cast::<u8>(), w * h * 4) };
    for (to, from) in pixels
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(rgba.as_chunks::<4>().0)
    {
        let alpha = u32::from(from[3]);
        let premultiplied = |channel: u8| (u32::from(channel) * alpha / 255) as u8;
        *to = [
            premultiplied(from[2]),
            premultiplied(from[1]),
            premultiplied(from[0]),
            from[3],
        ];
    }
    Some(bitmap)
}

/// Deletes a bitmap made by [`bitmap_from_rgba`].
pub(crate) fn delete_bitmap(bitmap: HBITMAP) {
    // SAFETY: the caller owns the bitmap and gives it up here.
    unsafe {
        let _ = DeleteObject(bitmap.into());
    }
}

/// A list of images of one size, for a toolbar or a list view. Destroyed
/// when dropped: it must outlive the control that shows it.
pub struct ImageList {
    handle: HIMAGELIST,
    size: i32,
}

impl ImageList {
    /// An empty list of images `size` pixels square.
    pub fn new(size: i32) -> ImageList {
        // SAFETY: the call takes plain numbers. A null list, which would
        // mean the system is out of resources, shows no images and is
        // harmless to add to and destroy.
        let handle = unsafe { ImageList_Create(size, size, ILC_COLOR32, 8, 8) };
        ImageList { handle, size }
    }

    /// Adds an image from `size * size * 4` bytes of red, green, blue and
    /// alpha. Returns its index, or `None` if it could not be added.
    pub fn add(&mut self, rgba: &[u8]) -> Option<usize> {
        let bitmap = bitmap_from_rgba(rgba, self.size, self.size)?;
        // SAFETY: the list and the bitmap are valid; the list copies the
        // bitmap, which is then deleted.
        let index = unsafe { ImageList_Add(self.handle, bitmap, None) };
        delete_bitmap(bitmap);
        usize::try_from(index).ok()
    }

    /// How many images the list holds.
    pub fn len(&self) -> usize {
        // SAFETY: the call takes any list handle.
        unsafe { ImageList_GetImageCount(self.handle) }.max(0) as usize
    }

    /// Whether the list holds no images.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The side of the images, in pixels.
    pub fn size(&self) -> i32 {
        self.size
    }

    pub(crate) fn handle(&self) -> HIMAGELIST {
        self.handle
    }
}

impl Drop for ImageList {
    fn drop(&mut self) {
        // SAFETY: the list was made by `new` and is destroyed nowhere
        // else.
        unsafe {
            let _ = ImageList_Destroy(Some(self.handle));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_are_added_and_counted() {
        let mut list = ImageList::new(16);
        assert!(list.is_empty());
        assert_eq!(list.size(), 16);
        let image = vec![200u8; 16 * 16 * 4];
        assert_eq!(list.add(&image), Some(0));
        assert_eq!(list.add(&image), Some(1));
        assert_eq!(list.len(), 2);
        // An image of the wrong size is refused.
        assert_eq!(list.add(&image[..100]), None);
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn a_bitmap_needs_exactly_its_pixels() {
        assert!(
            bitmap_from_rgba(&[0; 16], 2, 2)
                .map(delete_bitmap)
                .is_some()
        );
        assert!(bitmap_from_rgba(&[0; 15], 2, 2).is_none());
        assert!(bitmap_from_rgba(&[], 0, 0).is_none());
        assert!(bitmap_from_rgba(&[0; 16], -2, 2).is_none());
    }
}
