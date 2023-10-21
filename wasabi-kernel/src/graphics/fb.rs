//! utilites for framebuffer access

use core::slice;

use crate::graphics::Color;
use crate::mem::page_allocator::PageAllocator;
use crate::mem::structs::GuardedPages;
use crate::mem::structs::Mapped;
use crate::mem::structs::Unmapped;
use crate::mem::MemError;
use crate::prelude::UnwrapTicketLock;
use bootloader_api::info::FrameBuffer as BootFrameBuffer;
use bootloader_api::info::FrameBufferInfo;
use bootloader_api::info::PixelFormat;
use shared::lockcell::LockCell;
use x86_64::structures::paging::PageSize;
use x86_64::structures::paging::Size4KiB;
use x86_64::VirtAddr;

use super::Canvas;

pub(super) static HARDWEAR_FRAME_BUFFER: UnwrapTicketLock<Framebuffer> =
    unsafe { UnwrapTicketLock::new_uninit() };

/// gives access to the hardware framebuffer
pub fn framebuffer() -> &'static UnwrapTicketLock<Framebuffer> {
    &HARDWEAR_FRAME_BUFFER
}

/// The different memory sources for the framebuffer
#[derive(Debug)]
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
                let pages = *pages;
                *self = FramebufferSource::Dropped;
                Some(pages)
            }
            FramebufferSource::Dropped => None,
        }
    }
}

#[derive(Debug)]
/// A framebuffer used for rendering to the screen
pub struct Framebuffer {
    /// The start address of the framebuffer
    pub(super) start: VirtAddr,

    /// the source of the fb memory
    source: FramebufferSource,

    /// info about the framebuffer memory layout
    pub info: FrameBufferInfo,
}

impl Framebuffer {
    /// Allocates a new memory backed framebuffer
    pub fn alloc_new(info: FrameBufferInfo) -> Result<Self, MemError> {
        let page_count = (info.byte_len + Size4KiB::SIZE as usize - 1) / Size4KiB::SIZE as usize;

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

    /// Creates a new framebuffer for the given `vaddr`.
    ///
    /// # Safety
    ///
    /// `vaddr` must be a valid memory location with a lifetime of at least the result of
    /// this function, that cannot be accessed in any other way.
    pub unsafe fn new_at_virt_addr(vaddr: VirtAddr, info: FrameBufferInfo) -> Self {
        Framebuffer {
            start: vaddr,
            source: FramebufferSource::HardwareBuffer,
            info,
        }
    }

    /// Gives read access to the buffer
    pub fn buffer(&self) -> &[u8] {
        // Safety: buffer_start + byte_len is memory owned by this framebuffer
        unsafe { slice::from_raw_parts(self.start.as_ptr(), self.info.byte_len) }
    }

    /// Gives write access to the buffer
    pub fn buffer_mut(&mut self) -> &mut [u8] {
        unsafe { slice::from_raw_parts_mut(self.start.as_mut_ptr(), self.info.byte_len) }
    }
}

impl From<BootFrameBuffer> for Framebuffer {
    fn from(value: BootFrameBuffer) -> Self {
        // TODO use VirtAddr::from_slice once that is available
        let start = VirtAddr::new(value.buffer() as *const [u8] as *const u8 as u64);
        // Safety: start points to valid FB memory,
        // since we got it from the bootloader framebuffer
        unsafe { Self::new_at_virt_addr(start, value.info()) }
    }
}

impl Drop for Framebuffer {
    fn drop(&mut self) {
        if let Some(pages) = self.source.drop() {
            unsafe {
                // Safety: after drop, there are no ways to access the fb memory
                pages
                    .unmap_and_free()
                    .expect("failed to deallco framebuffer");
            }
        }
    }
}

impl Canvas for Framebuffer {
    fn clear(&mut self, color: Color) {
        let info = self.info;
        let buffer = self.buffer_mut();

        let mut line_pos = 0;
        for _y in 0..info.height {
            let mut pos = line_pos;
            for _x in 0..info.width {
                set_pixel_at_pos(buffer, pos, color, info.pixel_format);
                pos += info.bytes_per_pixel;
            }
            line_pos += info.stride * info.bytes_per_pixel;
        }
    }

    fn set_pixel(&mut self, x: u32, y: u32, c: Color) {
        let info = &self.info;
        let pos =
            info.bytes_per_pixel * info.stride * y as usize + info.bytes_per_pixel * x as usize;
        let format = info.pixel_format;
        set_pixel_at_pos(self.buffer_mut(), pos, c, format);
    }

    fn witdth(&self) -> u32 {
        self.info.width as u32
    }

    fn height(&self) -> u32 {
        self.info.height as u32
    }
}

/// Sets the pixel at index to color.
///
/// index is not the n'th pixel but the index in the `buffer` where the pixel
/// starts.
fn set_pixel_at_pos(buffer: &mut [u8], index: usize, color: Color, pixel_format: PixelFormat) {
    let r = color.r;
    let g = color.g;
    let b = color.b;
    match pixel_format {
        PixelFormat::Rgb => {
            buffer[index] = r;
            buffer[index + 1] = g;
            buffer[index + 2] = b;
        }
        PixelFormat::Bgr => {
            buffer[index] = b;
            buffer[index + 1] = g;
            buffer[index + 2] = r;
        }
        PixelFormat::U8 => {
            let gray = ((r as u32 + g as u32 + b as u32) / 3) as u8;
            buffer[index] = gray;
        }
        _ => panic!("unknown pixel format"),
    }
}

/// module containing startup/panic recovery functionality for the framebuffer
pub mod startup {
    use bootloader_api::info::{FrameBuffer, FrameBufferInfo, Optional};
    use x86_64::VirtAddr;

    use crate::boot_info;

    /// The start addr of the hardware framebuffer. Used during panic to recreate the fb
    pub static mut HARDWARE_FRAMEBUFFER_START_INFO: Option<(VirtAddr, FrameBufferInfo)> = None;

    /// Extracts the frambuffer from the boot info
    ///
    /// # Safety
    ///
    /// this is racy and must only be called while only a single execution has access
    pub unsafe fn take_boot_framebuffer() -> Option<FrameBuffer> {
        let boot_info = unsafe { boot_info() };
        let fb = core::mem::replace(&mut boot_info.framebuffer, Optional::None);
        fb.into_option()
    }
}
