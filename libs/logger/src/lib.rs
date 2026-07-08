#![no_std]

extern crate alloc;

#[cfg(not(feature = "no-color"))]
pub mod color;
pub mod dispatch;
mod own_logger;
mod ref_logger;

use core::{fmt::Write, marker::PhantomData};

#[cfg(not(feature = "no-color"))]
use color::{write_ansi_fg_color, Color, ANSI_RESET};

pub use dispatch::DispatchLogger;
use log::{LevelFilter, Record};
pub use own_logger::OwnLogger;
pub use ref_logger::RefLogger;
use shared::sync::CoreInfo;

pub struct FlushError {}

pub trait TryLog: Send + Sync {
    fn enabled(&self, metadata: &log::Metadata) -> bool;

    fn log(&self, record: &log::Record) -> Result<(), core::fmt::Error>;

    fn flush(&self) -> Result<(), FlushError>;
}

/// Trait used for the inital setup of a logger
pub trait LogRenameModuleSetup {
    /// rename modules before logging.
    ///
    /// The logger will replace `target` in the module of the log message with `rename`
    fn with_module_rename(&mut self, target: &'static str, rename: &'static str) -> &mut Self;

    /// Reserve internal space for `additional` renames
    ///
    /// This works analog to `Vec::reserve`
    fn reserve_renames(&mut self, additional: usize);
    /// Reserve internal space for `additional` renames
    ///
    /// This works analog to `Vec::reserve_exact`
    fn reserve_renames_exact(&mut self, additional: usize);
}

/// Trait used for the inital setup of a logger
///
/// If used in combination with [LogRenameModuleSetup] any modules should be
/// described by their original name. This trait will not follow renames.
pub trait LogModuleLevelSetup {
    /// Set the 'default' log level.
    ///
    /// You can override the default level for specific modules and their sub-modules using [`with_module_level`]
    fn with_level(&mut self, level: LevelFilter) -> &mut Self;

    /// Set the log level for the specified target module.
    ///
    /// This sets the log level of a specific module and all its sub-modules.
    /// When both the level for a parent module as well as a child module are set,
    /// the more specific value is taken. If the log level for the same module is
    /// specified twice, the resulting log level is implementation defined.
    fn with_module_level(&mut self, target: &'static str, level: LevelFilter) -> &mut Self;

    /// Reserve internal space for `additional` module levels
    ///
    /// This works analog to `Vec::reserve`
    fn reserve_module_levels(&mut self, additional: usize);
    /// Reserve internal space for `additional` module levels
    ///
    /// This works analog to `Vec::reserve_exact`
    fn reserve_module_levels_exact(&mut self, additional: usize);
}

/// Default colors for logging
///
/// Indexed by `level as usize`: Text, Error, Warn, Info, Debug, Trace
/// See [log::Level]
#[cfg(not(feature = "no-color"))]
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
    #[cfg(not(feature = "no-color"))]
    level_colors: &'a [Color; 6],

    module_rename_mapping: &'a [(&'a str, &'a str)],

    _core_info: PhantomData<CI>,
}

fn rule_matches(module_name: &str, metadata: &log::Metadata) -> bool {
    let target = metadata.target();
    target.starts_with(module_name)
        || (module_name.ends_with('!')
            && module_name.len() == target.len() + 1
            && module_name.starts_with(target))
}

fn write_record<W: Write, CI: CoreInfo>(
    writer: &mut W,
    record: &Record,
    opts: WriteOpts<'_, CI>,
) -> Result<(), core::fmt::Error> {
    write_processor::<W, CI>(writer)?;
    write_task_name::<W, CI>(writer)?;
    writer.write_char(' ')?;
    write_log_level(record, &opts, writer)?;
    writer.write_char(' ')?;
    write_target(record, opts, writer)?;
    writer.write_char(' ')?;

    writer.write_fmt(format_args!("{}\n", record.args()))?;

    Ok(())
}

fn write_task_name<W: Write, CI: CoreInfo>(writer: &mut W) -> Result<(), core::fmt::Error> {
    let ci = CI::instance();
    if ci.task_system_is_init() {
        unsafe {
            // Safety: task_system is initialized
            ci.write_current_task_name(writer)?;
        }
        writer.write_char(' ')?;
    }
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

#[cfg(not(feature = "no-color"))]
fn write_log_level<W: Write, CI>(
    record: &Record<'_>,
    opts: &WriteOpts<'_, CI>,
    writer: &mut W,
) -> Result<(), core::fmt::Error> {
    let level = record.level();
    let color = opts.level_colors[level as usize];
    write_ansi_fg_color(writer, color)?;
    writer.write_fmt(format_args!("{:<5}{}", level, ANSI_RESET))?;
    Ok(())
}

#[cfg(feature = "no-color")]
fn write_log_level<W: Write, CI>(
    record: &Record<'_>,
    _opts: &WriteOpts<'_, CI>,
    writer: &mut W,
) -> Result<(), core::fmt::Error> {
    let level = record.level();
    writer.write_fmt(format_args!("{:<5}", level))?;
    Ok(())
}
