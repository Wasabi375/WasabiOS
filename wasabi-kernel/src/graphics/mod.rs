//! Module containing all graphics related code for the kernel

pub mod canvas;
mod color;
pub mod fb;
pub mod kernel_font;
pub mod tty;

use alloc::boxed::Box;
use core::slice;
use log::{info, warn};
use logger::{dispatch::TargetLogger, OwnLogger};
use shared::sync::lockcell::LockCell;

use self::{
    canvas::CanvasWriter,
    fb::{
        startup::{take_boot_framebuffer, HARDWARE_FRAMEBUFFER_START_INFO},
        Framebuffer, HARDWARE_FRAMEBUFFER,
    },
    kernel_font::BitFont,
};
use crate::{
    core_local::CoreInterruptState,
    graphics::canvas::Canvas,
    logger::{setup_logger_module_rename, LOGGER},
    prelude::TicketLock,
};

pub use color::Color;

/// A point in 2D space
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Default)]
#[repr(C)]
#[allow(missing_docs)]
pub struct Point {
    pub x: u32,
    pub y: u32,
}

impl Point {
    /// creates a new 2d point
    pub fn new(x: u32, y: u32) -> Self {
        Point { x, y }
    }
}

impl AsRef<[u32]> for Point {
    fn as_ref(&self) -> &[u32] {
        unsafe { slice::from_raw_parts(&self.x as *const u32, 2) }
    }
}

impl AsMut<[u32]> for Point {
    fn as_mut(&mut self) -> &mut [u32] {
        unsafe { slice::from_raw_parts_mut(&mut self.x as *mut u32, 2) }
    }
}

/// Initializes graphics
///
/// # Safety
///
/// must only be called once during initialization.
/// Requires logging and heap access.
pub unsafe fn init(frambuffer_logger: bool) {
    let fb: Framebuffer = unsafe {
        take_boot_framebuffer()
            .expect("No framebuffer found")
            .into()
    };

    unsafe {
        HARDWARE_FRAMEBUFFER_START_INFO = Some((fb.start, fb.info.clone()));
    }

    *HARDWARE_FRAMEBUFFER.lock() = Some(fb);

    if frambuffer_logger {
        init_framebuffer_logger();
    }
}

/// Initializes logging to screen
///
/// # Safety
///
/// requires heap access
fn init_framebuffer_logger() {
    let mut fb = if let Some(fb) = HARDWARE_FRAMEBUFFER.lock().take() {
        fb
    } else {
        warn!("Framebuffer already taken. Frambuffer logger will not be created");
        return;
    };

    let bg_color = Color::BLACK;

    fb.clear(bg_color);

    let canvas_writer: CanvasWriter<_> = CanvasWriter::builder()
        .font(BitFont::get())
        .canvas(fb)
        .border_width(10)
        .border_height(10)
        .background_color(bg_color)
        .log_errors(false)
        .build()
        .expect("Canvas writer should be fully initialized");

    let canvas_lock = TicketLock::new_non_preemtable(canvas_writer);

    let mut fb_logger: OwnLogger<CanvasWriter<Framebuffer>, _, CoreInterruptState> =
        OwnLogger::new(canvas_lock);
    setup_logger_module_rename(&mut fb_logger);

    if let Some(dispatch_logger) = unsafe { LOGGER.as_ref() } {
        let logger = TargetLogger::new_secondary_boxed("framebuffer", Box::from(fb_logger));

        dispatch_logger.with_logger(logger)
    } else {
        panic!("No global logger found to register the framebuffer logger");
    }
    info!("Framebuffer Logger initialized");
}
