//! utilites for framebuffer access
// TODO rename file to frambuffer

use core::ptr::NonNull;

use crate::{graphics::Color, mem::MemError, prelude::TicketLock};
use alloc::boxed::Box;
use bootloader_api::info::{FrameBuffer as BootFrameBuffer, FrameBufferInfo, PixelFormat};

use super::canvas::Canvas;

/// The hardware backed framebuffer.
/// This can be taken, at which point this will be `None`.
pub static HARDWARE_FRAMEBUFFER: TicketLock<Option<Framebuffer>> = TicketLock::new(None);

/// The different memory sources for the framebuffer
#[derive(Debug)]
enum FramebufferSource {
    /// framebuffer is backed by the hardware buffer
    HardwareBuffer,
    /// framebuffer is backed by normal mapped memory
    Owned(#[allow(unused)] Box<[u8]>),
}

#[derive(Debug)]
/// A framebuffer used for rendering to the screen
pub struct Framebuffer {
    /// The start address of the framebuffer
    pub(super) buffer: NonNull<[u8]>,

    /// the source of the fb memory
    // This may be unused but is necessary to ensure the ownership over [buffer]
    _source: FramebufferSource,

    /// info about the framebuffer memory layout
    pub info: FrameBufferInfo,
}

unsafe impl Send for Framebuffer {}
unsafe impl Sync for Framebuffer {}

impl Framebuffer {
    /// Allocates a new memory backed framebuffer
    pub fn alloc_new(info: FrameBufferInfo) -> Result<Self, MemError> {
        let source_buffer = unsafe {
            // Safety: 0 is a valid byte
            Box::try_new_zeroed_slice(info.byte_len)?.assume_init()
        };

        let buffer = NonNull::from(source_buffer.as_ref());
        let source = FramebufferSource::Owned(source_buffer);

        Ok(Framebuffer {
            buffer,
            _source: source,
            info,
        })
    }

    /// Creates a new framebuffer for the given `vaddr`.
    ///
    /// # Safety
    ///
    /// `vaddr` must be a valid memory location with a lifetime of at least the result of
    /// this function, that cannot be accessed in any other way.
    pub unsafe fn new_hardware(buffer: NonNull<[u8]>, info: FrameBufferInfo) -> Self {
        Framebuffer {
            buffer,
            _source: FramebufferSource::HardwareBuffer,
            info,
        }
    }

    /// Gives read access to the buffer
    pub fn buffer(&self) -> &[u8] {
        // Safety: buffer_start + byte_len is memory owned by this framebuffer
        unsafe { self.buffer.as_ref() }
    }

    /// Gives write access to the buffer
    pub fn buffer_mut(&mut self) -> &mut [u8] {
        unsafe { self.buffer.as_mut() }
    }
}

impl From<BootFrameBuffer> for Framebuffer {
    fn from(value: BootFrameBuffer) -> Self {
        // Safety: start points to valid FB memory,
        // since we got it from the bootloader framebuffer
        unsafe { Self::new_hardware(value.buffer().into(), value.info()) }
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

    fn width(&self) -> u32 {
        self.info.width as u32
    }

    fn height(&self) -> u32 {
        self.info.height as u32
    }

    fn supports_scrolling() -> bool {
        true
    }

    fn scroll(
        &mut self,
        height: i32,
        clear_color: Color,
    ) -> core::result::Result<(), super::canvas::ScrollingNotSupportedError> {
        if height == 0 {
            return Ok(());
        }

        let lines_to_move = self.height() as usize - height.abs() as usize;

        let bytes_per_line = self.info.stride * self.info.bytes_per_pixel;
        let (start, dest, clear_start): (usize, usize, u32) = if height.is_positive() {
            // move every line up by height pixels. therefor we copy starting
            // from the nth line and copy into the 0th line
            (height as usize * bytes_per_line, 0, lines_to_move as u32)
        } else {
            // move every line down by height pixels. therefor we copy starting
            // from the 0th line and copy into the nth line
            (0, height.abs() as usize * bytes_per_line, 0)
        };
        let src = start as usize..(start + lines_to_move * bytes_per_line) as usize;

        self.buffer_mut().copy_within(src, dest);

        // clear the freed up lines
        for line in 0..height.abs() as u32 {
            let y = clear_start + line;
            for x in 0..self.width() {
                self.set_pixel(x, y, clear_color);
            }
        }
        Ok(())
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
    use core::ptr::NonNull;

    use bootloader_api::info::{FrameBuffer, FrameBufferInfo, Optional};

    use crate::kernel_info::KernelInfo;

    /// The buffer and info of the hardware framebuffer. Used during panic to recreate the fb
    pub static mut HARDWARE_FRAMEBUFFER_START_INFO: Option<(NonNull<[u8]>, FrameBufferInfo)> = None;

    /// Extracts the frambuffer from the boot info
    ///
    /// # Safety
    ///
    /// Must only be called on bsp during startup
    pub fn take_boot_framebuffer() -> Option<FrameBuffer> {
        let fb = core::mem::replace(
            &mut KernelInfo::get_mut_for_bsp().boot_info.framebuffer,
            Optional::None,
        );
        fb.into_option()
    }
}
