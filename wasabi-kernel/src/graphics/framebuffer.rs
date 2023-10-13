//! utilites for framebuffer access

use core::slice;

use crate::graphics::Color;
use crate::mem::page_allocator;
use crate::mem::page_allocator::PageAllocator;
use crate::mem::structs::GuardedPages;
use crate::mem::structs::Mapped;
use crate::mem::structs::Unmapped;
use crate::mem::MemError;
use bootloader_api::info::FrameBuffer as BootFrameBuffer;
use bootloader_api::info::FrameBufferInfo;
use bootloader_api::info::PixelFormat;
use shared::lockcell::LockCell;
use x86_64::structures::paging::Size4KiB;
use x86_64::VirtAddr;

enum FramebufferSource {
    /// framebuffer is backed by the hardware buffer
    HardwareBuffer,
    /// framebuffer is backed by normal mapped memory
    Owned(Mapped<GuardedPages<Size4KiB>>),
    /// framebuffer is dropped
    Dropped,
}

impl FramebufferSource {
    fn drop(&mut self) -> Option<Mapped<GuardedPages<Size4KiB>>> {
        match self {
            FramebufferSource::HardwareBuffer => None,
            FramebufferSource::Owned(pages) => {
                self = &mut FramebufferSource::Dropped;
                Some(pages)
            }
            FramebufferSource::Dropped => None,
        }
    }
}

pub struct Framebuffer {
    /// The start address of the framebuffer
    start: VirtAddr,

    /// the source of the fb memory
    source: FramebufferSource,

    pub info: FrameBufferInfo,
}

impl Framebuffer {
    pub fn alloc_new(info: FrameBufferInfo) -> Result<Self, MemError> {
        let page_count = todo!("calculate page count");

        let pages = PageAllocator::get_kernel_allocator()
            .lock()
            .allocate_guarded_pages(page_count, true, true)?;

        let pages = Unmapped(pages);
        let mapped_pages = pages.alloc_and_map()?;
        let start = mapped_pages.0.start_addr();

        let source = FramebufferSource::Owned(mapped_pages);

        Ok(Framebuffer {
            start,
            source,
            info,
        })
    }

    pub fn buffer(&self) -> &[u8] {
        // Safety: buffer_start + byte_len is memory owned by this framebuffer
        unsafe { slice::from_raw_parts(self.start.as_ptr(), self.byte_len) }
    }

    pub fn buffer_mut(&mut self) -> &mut [u8] {
        unsafe { slice::from_raw_parts_mut(self.start.as_mut_ptr(), self.byte_len) }
    }
}

impl From<BootFrameBuffer> for Framebuffer {
    fn from(value: BootFrameBuffer) -> Self {
        let start = VirtAddr::from_ptr(value.buffer());
        let info = value.info();
        Framebuffer {
            start: (),
            source: FramebufferSource::HardwareBuffer,
            info,
        }
    }
}

impl Drop for Framebuffer {
    fn drop(&mut self) {
        if let Some(pages) = self.source.drop() {
            unsafe {
                // Safety: after drop, there are no ways to access the fb memory
                pages
                    .unmap_and_dealloc()
                    .expect("failed to deallco framebuffer");
            }
        }
    }
}

/// clears the framebuffer to the given color
///
/// uses [clear_frame_buffer_rgb] internally
pub fn clear_frame_buffer(fb: &mut BootFrameBuffer, c: Color) {
    clear_frame_buffer_rgb(fb, c.r, c.g, c.b)
}

/// clears the framebuffer to the given rgb color
pub fn clear_frame_buffer_rgb(fb: &mut BootFrameBuffer, r: u8, g: u8, b: u8) {
    let info = fb.info();
    let buffer = fb.buffer_mut();

    for y in 0..info.height {
        for x in 0..info.width {
            let pos = info.bytes_per_pixel * info.stride * y + x * info.bytes_per_pixel;
            match info.pixel_format {
                bootloader_api::info::PixelFormat::Rgb => {
                    buffer[pos] = r;
                    buffer[pos + 1] = g;
                    buffer[pos + 2] = b;
                }
                bootloader_api::info::PixelFormat::Bgr => {
                    buffer[pos] = b;
                    buffer[pos + 1] = g;
                    buffer[pos + 2] = r;
                }
                bootloader_api::info::PixelFormat::U8 => {
                    let gray = ((r as u32 + g as u32 + b as u32) / 3) as u8;
                    buffer[pos] = gray;
                }
                _ => panic!("unknown pixel format"),
            }
        }
    }
}
