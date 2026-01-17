//! A logger that prints all messages with a simple, readable output format.
//!
//! From: `<https://github.com/borntyping/rust-simple_logger/blob/main/src/lib.rs>`
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

use crate::{write_record, LogModuleLevelSetup, LogRenameModuleSetup, TryLog, WriteOpts};
use alloc::vec::Vec;
use core::{
    fmt::{self, Error, Write},
    marker::PhantomData,
};
use log::{Level, LevelFilter, Log, Metadata, Record, SetLoggerError};
use shared::sync::{lockcell::LockCell, CoreInfo};

#[cfg(not(feature = "no-color"))]
use crate::{color::Color, default_colors};

pub struct RefLogger<'a, W, L, CI> {
    /// The default logging level
    default_level: LevelFilter,

    /// The specific logging level for each module
    ///
    /// This is used to override the default value for some specific modules.
    /// After initialization, the vector is sorted so that the first (prefix) match
    /// directly gives us the desired log level.
    module_levels: Vec<(&'a str, LevelFilter)>,

    /// a list off mappings renaming modules
    ///
    /// This can be used to shorten module names
    module_rename_mapping: Vec<(&'a str, &'a str)>,

    writer: &'a L,
    _phantom_writer: PhantomData<W>,

    _phantom_core_info: PhantomData<CI>,

    #[cfg(not(feature = "no-color"))]
    level_colors: [Color; 6],
}

unsafe impl<W, L: Sync, CI> Sync for RefLogger<'_, W, L, CI> {}
unsafe impl<W, L: Send, CI> Send for RefLogger<'_, W, L, CI> {}

impl<'a, W, L: LockCell<W>, CI: CoreInfo> RefLogger<'a, W, L, CI>
where
    W: fmt::Write,
{
    #[must_use]
    pub fn new(writer: &'a L) -> Self {
        RefLogger {
            // default is trace, because level filtering is done in dispatch logger
            default_level: LevelFilter::Trace,
            module_levels: Vec::new(),
            module_rename_mapping: Vec::new(),
            writer,
            _phantom_writer: PhantomData,
            _phantom_core_info: PhantomData,

            #[cfg(not(feature = "no-color"))]
            level_colors: default_colors(),
        }
    }

    /// Overrides the log color used for the specified level.
    #[cfg(not(feature = "no-color"))]
    #[must_use]
    pub fn with_level_color(mut self, level: Level, color: Color) -> Self {
        self.level_colors[level as usize] = color;

        self
    }
}

impl<'a, W: Write, L: LockCell<W>, CI: CoreInfo> RefLogger<'a, W, L, CI> {
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

impl<'a, W: Write, L: LockCell<W>, CI: CoreInfo> RefLogger<'a, W, L, CI> {
    pub fn try_log(&self, record: &Record) -> Result<(), Error> {
        if !self.enabled(record.metadata()) {
            return Ok(());
        }

        let opts = WriteOpts::<'_, CI> {
            #[cfg(not(feature = "no-color"))]
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

impl<'a, W: Write, L: LockCell<W>, CI: CoreInfo> Log for RefLogger<'a, W, L, CI> {
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

impl<'a, W: Write, L: LockCell<W>, CI: CoreInfo> TryLog for RefLogger<'a, W, L, CI> {
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

impl<'a, W: Write, L: LockCell<W>, CI: CoreInfo> LogModuleLevelSetup for RefLogger<'a, W, L, CI> {
    fn with_level(&mut self, level: LevelFilter) -> &mut Self {
        self.default_level = level;
        self
    }

    fn with_module_level(&mut self, target: &'static str, level: LevelFilter) -> &mut Self {
        if let Some(old_position) = self.module_levels.iter().position(|(t, _)| *t == target) {
            self.module_levels.swap_remove(old_position);
        }
        self.module_levels.push((target, level));
        self
    }

    fn reserve_module_levels(&mut self, additional: usize) {
        self.module_levels.reserve(additional);
    }

    fn reserve_module_levels_exact(&mut self, additional: usize) {
        self.module_levels.reserve_exact(additional);
    }
}

impl<'a, W: Write, L: LockCell<W>, CI: CoreInfo> LogRenameModuleSetup for RefLogger<'a, W, L, CI> {
    fn with_module_rename(&mut self, target: &'static str, rename: &'static str) -> &mut Self {
        self.module_rename_mapping.push((target, rename));

        self
    }

    fn reserve_renames(&mut self, additional: usize) {
        self.module_rename_mapping.reserve(additional);
    }

    fn reserve_renames_exact(&mut self, additional: usize) {
        self.module_rename_mapping.reserve_exact(additional);
    }
}
