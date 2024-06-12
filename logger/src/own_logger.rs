//! A logger that prints all messages with a simple, readable output format.
//!
//! ```rust
//! simple_logger::StaticLogger::new().env().init().unwrap();
//!
//! log::warn!("This is an example message.");
//! ```
//!
//! Some shortcuts are available for common use cases.
//!
//! Just initialize logging without any configuration:
//!
//! ```rust
//! simple_logger::init().unwrap();
//! ```
//!
//! Hardcode a default log level:
//!
//! ```rust
//! simple_logger::init_with_level(log::Level::Warn).unwrap();
//! ```
//!
//! From: `<https://github.com/borntyping/rust-simple_logger/blob/main/src/lib.rs>`
//!
//! Original Version:
//! Copyright 2015-2021 Sam Clements
//!
//! Permission is hereby granted, free of charge, to any person obtaining a copy
//! of this software and associated documentation files (the "Software"),
//! to deal in the Software without restriction, including without limitation
//! the rights to use, copy, modify, merge, publish, distribute, sublicense,
//! and/or sell copies of the Software, and to permit persons to whom
//! the Software is furnished to do so, subject to the following conditions:
//!
//! The above copyright notice and this permission notice shall be included in
//! all copies or substantial portions of the Software.
//!
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
//! EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
//! MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
//! IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM,
//! DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR
//! OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE
//! USE OR OTHER DEALINGS IN THE SOFTWARE.

use crate::{default_colors, write_record, LogSetup, TryLog, WriteOpts};
use core::{
    fmt::{self, Error, Write},
    marker::PhantomData,
};
use log::{Level, LevelFilter, Log, Metadata, Record, SetLoggerError};
use shared::{lockcell::LockCell, CoreInfo};
use staticvec::StaticVec;

#[cfg(feature = "color")]
use colored::Color;

/// Implements [`Log`] and a set of simple builder methods for configuration.
///
// TODO OwnLogger Docs are outdated
//  they reflect API and functionality of the SimpleLogger this is based off
///
/// Use the various "builder" methods on this struct to configure the logger,
/// then call [`init`](StaticLogger::init) to configure the [`log`] crate.
///
/// This logger does not use any heap allocations. All data is stored in place.
/// It can only store [`LevelFilter`]s for `N` log targets/modules and `R` rename mappings
// TODO unifiy with RefLogger:
//      I could use RefOrBox. This would require boxing the lock to the writer
pub struct OwnLogger<W, L, CI, const N: usize = 126, const R: usize = 126> {
    /// The default logging level
    default_level: LevelFilter,

    /// The specific logging level for each module
    ///
    /// This is used to override the default value for some specific modules.
    /// After initialization, the vector is sorted so that the first (prefix) match
    /// directly gives us the desired log level.
    module_levels: StaticVec<(&'static str, LevelFilter), N>,

    /// a list off mappings renaming modules
    ///
    /// This can be used to shorten module names
    module_rename_mapping: StaticVec<(&'static str, &'static str), R>,

    writer: L,
    _phantom_writer: PhantomData<W>,

    _phantom_core_info: PhantomData<CI>,

    #[cfg(feature = "color")]
    level_colors: [Color; 6],
}

unsafe impl<W, L: Sync, CI, const N: usize, const R: usize> Sync for OwnLogger<W, L, CI, N, R> {}
unsafe impl<W, L: Send, CI, const N: usize, const R: usize> Send for OwnLogger<W, L, CI, N, R> {}

impl<W, L, CI: CoreInfo, const N: usize, const R: usize> OwnLogger<W, L, CI, N, R>
where
    W: fmt::Write,
    L: LockCell<W>,
    CI: CoreInfo,
{
    /// Initializes the global logger with a StaticLogger instance with
    /// default log level set to `Level::Trace`.
    ///
    /// ```no_run
    /// use simple_logger::StaticLogger;
    /// StaticLogger::new().env().init().unwrap();
    /// log::warn!("This is an example message.");
    /// ```
    ///
    /// [`init`]: #method.init
    // TODO fix must use message
    #[must_use = "You must call init() to begin logging"]
    pub fn new(writer: L) -> Self {
        OwnLogger {
            // default is trace because level filtering is mainly done in dispatch logger
            default_level: LevelFilter::Trace,
            module_levels: StaticVec::new(),
            module_rename_mapping: StaticVec::new(),
            writer,
            _phantom_writer: PhantomData,
            _phantom_core_info: PhantomData,
            #[cfg(feature = "color")]
            level_colors: default_colors(),
        }
    }

    /// Set the 'default' log level.
    ///
    /// You can override the default level for specific modules and their sub-modules using [`with_module_level`]
    ///
    /// This must be called before [`env`]. If called after [`env`], it will override the value loaded from the environment.
    ///
    /// [`env`]: #method.env
    /// [`with_module_level`]: #method.with_module_level
    #[must_use = "You must call init() to begin logging"]
    pub fn with_level(mut self, level: LevelFilter) -> Self {
        self.default_level = level;
        self
    }

    /// Override the log level for some specific modules.
    ///
    /// This sets the log level of a specific module and all its sub-modules.
    /// When both the level for a parent module as well as a child module are set,
    /// the more specific value is taken. If the log level for the same module is
    /// specified twice, the resulting log level is implementation defined.
    ///
    /// # Examples
    ///
    /// Silence an overly verbose crate:
    ///
    /// ```no_run
    /// use simple_logger::StaticLogger;
    /// use log::LevelFilter;
    ///
    /// StaticLogger::new().with_module_level("chatty_dependency", LevelFilter::Warn).init().unwrap();
    /// ```
    ///
    /// Disable logging for all dependencies:
    ///
    /// ```no_run
    /// use simple_logger::StaticLogger;
    /// use log::LevelFilter;
    ///
    /// StaticLogger::new()
    ///     .with_level(LevelFilter::Off)
    ///     .with_module_level("my_crate", LevelFilter::Info)
    ///     .init()
    ///     .unwrap();
    /// ```
    #[must_use = "You must call init() to begin logging"]
    pub fn with_module_level(mut self, target: &'static str, level: LevelFilter) -> Self {
        self.module_levels.push((target, level));

        /* Normally this is only called in `init` to avoid redundancy, but we can't initialize the logger in tests */
        #[cfg(test)]
        self.module_levels
            .sort_by_key(|(name, _level)| name.len().wrapping_neg());

        self
    }

    /// Overrides the log color used for the specified level.
    #[cfg(feature = "color")]
    #[must_use = "You must call init() to begin logging"]
    pub fn with_level_color(mut self, level: Level, color: Color) -> Self {
        self.level_colors[level as usize] = color;

        self
    }
}

impl<W: Write, L: LockCell<W>, CI: CoreInfo, const N: usize, const R: usize>
    OwnLogger<W, L, CI, N, R>
{
    /// 'Init' the actual logger, instantiate it and configure it,
    pub fn init(&mut self) {
        /* Sort all module levels from most specific to least specific. The length of the module
         * name is used instead of its actual depth to avoid module name parsing.
         */
        self.module_levels
            .sort_unstable_by_key(|(name, _level)| name.len().wrapping_neg());
        let max_level = self
            .module_levels
            .iter()
            .map(|(_name, level)| level)
            .copied()
            .max();
        let max_level = max_level
            .map(|lvl| lvl.max(self.default_level))
            .unwrap_or(self.default_level);
        log::set_max_level(max_level);
    }

    /// Sets this logger as the global logger.
    ///
    /// This calls [log::set_logger] and can therefor only be called once on
    /// any [Log] implementation
    pub fn set_globally(&'static mut self) -> Result<(), SetLoggerError> {
        log::set_logger(self)
    }
}

impl<W: Write, L: LockCell<W>, CI: CoreInfo, const N: usize, const R: usize>
    OwnLogger<W, L, CI, N, R>
{
    pub fn try_log(&self, record: &Record) -> Result<(), Error> {
        if !self.enabled(record.metadata()) {
            return Ok(());
        }

        let opts = WriteOpts::<'_, CI> {
            level_colors: &self.level_colors,
            module_rename_mapping: &self.module_rename_mapping,
            _core_info: PhantomData,
        };
        let mut writer = self.writer.lock();

        write_record(writer.as_mut(), record, opts)
    }

    pub fn enabled(&self, metadata: &Metadata) -> bool {
        &metadata.level().to_level_filter()
            <= self
                .module_levels
                .iter()
                /* At this point the vec is already sorted so that we can simply take
                 * the first match
                 */
                .find(|(name, _level)| metadata.target().starts_with(name))
                .map(|(_name, level)| level)
                .unwrap_or(&self.default_level)
    }
}

impl<W: Write, L: LockCell<W>, CI: CoreInfo, const N: usize, const R: usize> Log
    for OwnLogger<W, L, CI, N, R>
{
    fn enabled(&self, metadata: &Metadata) -> bool {
        self.enabled(metadata)
    }

    fn log(&self, record: &Record) {
        if let Err(e) = self.try_log(record) {
            panic!("StaticLogger failed to write to output: {e:?}");
        }
    }

    fn flush(&self) {}
}

impl<W: Write, L: LockCell<W>, CI: CoreInfo, const N: usize, const R: usize> TryLog
    for OwnLogger<W, L, CI, N, R>
{
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        self.enabled(metadata)
    }

    fn log(&self, record: &log::Record) -> Result<(), core::fmt::Error> {
        self.try_log(record)
    }

    fn flush(&self) -> Result<(), crate::FlushError> {
        Ok(())
    }
}

impl<W, L, CI: CoreInfo, const N: usize, const R: usize> LogSetup for OwnLogger<W, L, CI, N, R> {
    fn with_module_rename(&mut self, target: &'static str, rename: &'static str) -> &mut Self {
        self.module_rename_mapping.push((target, rename));

        self
    }
}
