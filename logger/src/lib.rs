#![no_std]

extern crate alloc;

#[cfg(feature = "color")]
pub mod color;
pub mod dispatch;
mod own_logger;
mod ref_logger;

use core::{fmt::Write, marker::PhantomData};

#[cfg(feature = "color")]
use color::{write_ansi_fg_color, Color, ANSI_RESET};

pub use dispatch::DispatchLogger;
use log::Record;
pub use own_logger::OwnLogger;
pub use ref_logger::RefLogger;
use shared::sync::CoreInfo;

pub struct FlushError {}

pub trait TryLog: Send + Sync {
    fn enabled(&self, metadata: &log::Metadata) -> bool;

    fn log(&self, record: &log::Record) -> Result<(), core::fmt::Error>;

    fn flush(&self) -> Result<(), FlushError>;
}

pub trait LogSetup {
    fn with_module_rename(&mut self, target: &'static str, rename: &'static str) -> &mut Self;
}

/// Default colors for logging
///
/// Indexed by `level as usize`: Text, Error, Warn, Info, Debug, Trace
/// See [log::Level]
pub const fn default_colors() -> [Color; 6] {
    [
        Color::White,
        Color::Red,
        Color::Yellow,
        Color::Green,
        Color::Blue,
        Color::White,
    ]
}

struct WriteOpts<'a, CI> {
    #[cfg(feature = "color")]
    level_colors: &'a [Color; 6],

    module_rename_mapping: &'a [(&'a str, &'a str)],

    _core_info: PhantomData<CI>,
}

fn write_record<W: Write, CI: CoreInfo>(
    writer: &mut W,
    record: &Record,
    opts: WriteOpts<'_, CI>,
) -> Result<(), core::fmt::Error> {
    write_processor::<W, CI>(writer)?;
    writer.write_char(' ')?;
    write_log_level(record, &opts, writer)?;
    writer.write_char(' ')?;
    write_target(record, opts, writer)?;
    writer.write_char(' ')?;

    writer.write_fmt(format_args!("{}\n", record.args()))?;

    Ok(())
}

fn write_processor<W: Write, CI: CoreInfo>(writer: &mut W) -> Result<(), core::fmt::Error> {
    if CI::s_is_bsp() {
        writer.write_str("(BSP)")
    } else {
        writer.write_fmt(format_args!("(AP: {})", CI::s_core_id()))
    }
}

fn write_target<W: Write, CI>(
    record: &Record<'_>,
    opts: WriteOpts<'_, CI>,
    writer: &mut W,
) -> Result<(), core::fmt::Error> {
    let target = if !record.target().is_empty() {
        record.target()
    } else {
        record.module_path().unwrap_or_default()
    };
    let mut target_renamed = false;
    for (old_name, new_name) in opts.module_rename_mapping {
        if target.starts_with(old_name) {
            target_renamed = true;
            let rest = &target[old_name.len()..];
            writer.write_fmt(format_args!(" [{}{}]", new_name, rest))?;
            break;
        }
    }
    Ok(if !target_renamed {
        writer.write_fmt(format_args!("[{target}]"))?;
    })
}

fn write_log_level<W: Write, CI>(
    record: &Record<'_>,
    opts: &WriteOpts<'_, CI>,
    writer: &mut W,
) -> Result<(), core::fmt::Error> {
    let level = record.level();
    if cfg!(feature = "color") {
        let color = opts.level_colors[level as usize];
        write_ansi_fg_color(writer, color)?;
        writer.write_fmt(format_args!("{:<5}{}", level, ANSI_RESET))?;
    } else {
        writer.write_fmt(format_args!("{:<5}", level))?;
    };
    Ok(())
}
