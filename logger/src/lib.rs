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

#![no_std]

use core::{fmt::Write, marker::PhantomData};
use log::{Level, LevelFilter, Log, Metadata, Record, SetLoggerError};
use shared::lockcell::LockCell;
use staticvec::StaticVec;

#[cfg(feature = "color")]
use colored::{Color, ColoredString, Colorize};

/// Implements [`Log`] and a set of simple builder methods for configuration.
///
/// Use the various "builder" methods on this struct to configure the logger,
/// then call [`init`](StaticLogger::init) to configure the [`log`] crate.
///
/// This logger does not use any heap allocations. All data is stored in place.
/// It can only store [`LevelFilter`]s for `N` log targets/modules.
pub struct StaticLogger<'a, W, L, const N: usize = 256> {
    /// The default logging level
    default_level: LevelFilter,

    /// The specific logging level for each module
    ///
    /// This is used to override the default value for some specific modules.
    /// After initialization, the vector is sorted so that the first (prefix) match
    /// directly gives us the desired log level.
    module_levels: StaticVec<(&'a str, LevelFilter), N>,

    writer: &'a L,
    _phantom_writer: PhantomData<W>,

    #[cfg(feature = "color")]
    level_colors: [Color; 6],
}

unsafe impl<W, L: Sync, const N: usize> Sync for StaticLogger<'_, W, L, N> {}
unsafe impl<W, L: Send, const N: usize> Send for StaticLogger<'_, W, L, N> {}

const fn default_colors() -> [Color; 6] {
    [
        Color::White,
        Color::Red,
        Color::Yellow,
        Color::Green,
        Color::Blue,
        Color::White,
    ]
}

impl<'a, W, L: LockCell<W>> StaticLogger<'a, W, L> {
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
    #[must_use = "You must call init() to begin logging"]
    pub const fn new(writer: &'a L) -> StaticLogger<'a, W, L> {
        StaticLogger {
            default_level: LevelFilter::Info,
            module_levels: StaticVec::new(),
            writer,
            _phantom_writer: PhantomData,
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
    pub fn with_level(mut self, level: LevelFilter) -> StaticLogger<'a, W, L> {
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
    pub fn with_module_level(
        mut self,
        target: &'static str,
        level: LevelFilter,
    ) -> StaticLogger<'a, W, L> {
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
    pub fn with_level_color(mut self, level: Level, color: Color) -> StaticLogger<'a, W, L> {
        self.level_colors[level as usize] = color;

        self
    }
}
impl<'a, W: Write, L: LockCell<W>> StaticLogger<'a, W, L> {
    /// 'Init' the actual logger, instantiate it and configure it,
    /// this method MUST be called in order for the logger to be effective.
    pub fn init(&'static mut self) -> Result<(), SetLoggerError> {
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
        log::set_logger(self)
    }
}

impl<'a, W: Write, L: LockCell<W>> Log for StaticLogger<'a, W, L> {
    fn enabled(&self, metadata: &Metadata) -> bool {
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

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            let mut write = self.writer.lock();

            let level = record.level();
            let write_result = if cfg!(feature = "color") {
                let color = self.level_colors[level as usize];
                let level: ColoredString<50> = level
                    .as_str()
                    .color(color)
                    .expect("StaticString of size 50 should be enough for the level");

                write.write_fmt(format_args!("{:<5}", level))
            } else {
                write.write_fmt(format_args!("{:<5}", level))
            };
            if write_result.is_err() {
                drop(write);
                panic!("Failed to write to serial port!");
            }

            let target = if !record.target().is_empty() {
                record.target()
            } else {
                record.module_path().unwrap_or_default()
            };

            if write
                .write_fmt(format_args!(" [{target}] {}\n", record.args()))
                .is_err()
            {
                drop(write);
                panic!("Failed to write to serial port!");
            }
        }
    }

    fn flush(&self) {}
}
