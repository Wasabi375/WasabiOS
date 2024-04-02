//! A logger that fowards log calls to other loggers
//!
//! This does not depend on the alloc carate.

use core::fmt::Display;

use log::{Level, LevelFilter, Log, Metadata, Record, SetLoggerError};
use shared::lockcell::{InterruptState, RWLockCell, ReadWriteCell};
use staticvec::StaticVec;

use crate::{FlushError, TryLog};

#[cfg(feature = "alloc")]
use shared::reforbox::RefOrBox;
#[cfg(not(feature = "alloc"))]
type RefOrBox<'a, T> = &'a T;

#[cfg(feature = "alloc")]
use alloc::boxed::Box;

/// Contains a reference to a [TryLogger] and some additional metadata
/// for [DispatchLogger].
pub struct TargetLogger<'a> {
    /// identifier string used for logging. Displayed to the user
    pub name: &'a str,
    /// The logger reference
    pub logger: RefOrBox<'a, dyn TryLog + Sync + 'a>,
    /// `true` if the [DispatchLogger] should panic if this errors
    pub panic_on_error: bool,
    /// `true` if the [DispatchLogger] should panic if this errors on flush
    pub panic_on_flush_error: bool,
    /// if `true` errors in other loggers are logged in this logger
    pub primary: bool,
}

impl<'a> TargetLogger<'a> {
    pub fn new_primary(name: &'a str, logger: &'a (dyn TryLog + Sync)) -> Self {
        #[cfg(not(feature = "alloc"))]
        let logger = logger;
        #[cfg(feature = "alloc")]
        let logger = RefOrBox::Ref(logger);
        Self {
            name,
            logger,
            panic_on_error: true,
            panic_on_flush_error: true,
            primary: true,
        }
    }

    pub fn new_secondary(name: &'a str, logger: &'a (dyn TryLog + Sync)) -> Self {
        #[cfg(not(feature = "alloc"))]
        let logger = logger;
        #[cfg(feature = "alloc")]
        let logger = RefOrBox::Ref(logger);
        Self {
            name,
            logger,
            panic_on_error: false,
            panic_on_flush_error: false,
            primary: false,
        }
    }

    #[cfg(feature = "alloc")]
    pub fn new_primary_boxed(name: &'a str, logger: Box<(dyn TryLog + Sync)>) -> Self {
        let logger = RefOrBox::Boxed(logger);
        Self {
            name,
            logger,
            panic_on_error: true,
            panic_on_flush_error: true,
            primary: true,
        }
    }

    #[cfg(feature = "alloc")]
    pub fn new_secondary_boxed(name: &'a str, logger: Box<(dyn TryLog + Sync)>) -> Self {
        let logger = RefOrBox::Boxed(logger);
        Self {
            name,
            logger,
            panic_on_error: false,
            panic_on_flush_error: false,
            primary: false,
        }
    }

    fn logger(&'a self) -> &'a (dyn TryLog + Sync) {
        #[cfg(feature = "alloc")]
        match self.logger {
            RefOrBox::Ref(r) => r,
            RefOrBox::Boxed(ref b) => b.as_ref(),
        }
        #[cfg(not(feature = "alloc"))]
        self.logger
    }

    fn enabled(&self, metadata: &Metadata) -> bool {
        self.logger().enabled(metadata)
    }

    fn log(&self, record: &Record) -> Result<(), core::fmt::Error> {
        self.logger().log(record)
    }

    fn flush(&self) -> Result<(), FlushError> {
        self.logger().flush()
    }
}

/// A logger implementation that dispatches all log calls to multiple target loggers.
pub struct DispatchLogger<'a, I, const N: usize = 2, const L: usize = 126> {
    /// The default logging level
    default_level: LevelFilter,

    /// the loggers this logger dispatches log calls to
    loggers: ReadWriteCell<StaticVec<TargetLogger<'a>, N>, I>,

    /// The specific logging level for each module
    ///
    /// This is used to override the default value for some specific modules.
    /// After initialization, the vector is sorted so that the first (prefix) match
    /// directly gives us the desired log level.
    module_levels: StaticVec<(&'a str, LevelFilter), L>,
}

// TODO implement TryLog for DispatchLogger
//      should error where the current log impl panics
//      and just silently log other erros

impl<'a, const N: usize, const L: usize, I: InterruptState> DispatchLogger<'a, I, N, L> {
    /// Creates a new dispatch logger
    pub fn new() -> Self {
        DispatchLogger {
            default_level: LevelFilter::Info,
            loggers: ReadWriteCell::new(StaticVec::new()),
            module_levels: StaticVec::new(),
        }
    }

    /// Adds a target logger
    ///
    /// Unlike other setup functions this can be used while the logger is in use.
    pub fn with_logger(&self, target_logger: TargetLogger<'a>) {
        self.loggers.write().push(target_logger);
    }

    /// removes all target loggers
    pub fn clear_loggers(&self) {
        self.loggers.write().clear();
    }

    /// Removes the given target logger
    pub fn remove_logger(&self, target_logger: &'a TargetLogger) {
        self.loggers
            .write()
            .retain(|l| l as *const TargetLogger<'a> != target_logger as *const TargetLogger<'_>);
    }

    /// Set the 'default' log level.
    ///
    /// You can override the default level for specific modules and their sub-modules using [`with_module_level`]
    ///
    /// [`with_module_level`]: #method.with_module_level
    pub fn with_level(mut self, level: LevelFilter) -> Self {
        self.default_level = level;
        self
    }

    /// Set the log level for the specified target module.
    pub fn with_module_level(mut self, target: &'static str, level: LevelFilter) -> Self {
        self.module_levels.push((target, level));
        self
    }

    /// 'Init' the actual logger, instantiate it and configure it,
    pub fn init(&mut self) {
        /* Sort all module levels from most specific to least specific. The length of the module
         * name is used instead of its actual depth to avoid module name parsing.
         */
        self.module_levels
            .sort_unstable_by_key(|(name, _level)| name.len().wrapping_neg());
    }

    /// Sets this logger as the global logger.
    ///
    /// This calls [log::set_logger] and can therefor only be called once on
    /// any [Log] implementation
    pub fn set_globally(&'static mut self) -> Result<(), SetLoggerError> {
        log::set_logger(self)
    }
}

impl<'a, const N: usize, const L: usize, I: InterruptState> Log for DispatchLogger<'a, I, N, L> {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        if &metadata.level().to_level_filter()
            > self
                .module_levels
                .iter()
                /* At this point the vec is already sorted so that we can simply take
                 * the first match
                 */
                .find(|(name, _level)| metadata.target().starts_with(name))
                .map(|(_name, level)| level)
                .unwrap_or(&self.default_level)
        {
            return false;
        }

        self.loggers.read().iter().any(|l| l.enabled(metadata))
    }

    fn log(&self, record: &log::Record) {
        let mut error_to_panic = StaticVec::<&'a str, N>::new();
        let mut error_to_log = StaticVec::<&'a str, N>::new();

        for logger in self.loggers.read().iter() {
            if logger.log(record).is_err() {
                if logger.panic_on_error {
                    error_to_panic.push(logger.name);
                }
                error_to_log.push(logger.name);
            }
        }

        if error_to_log.is_not_empty() {
            self.log_log_error(
                format_args!("to log {} message", record.level()),
                &error_to_log,
            );
        }

        if error_to_panic.is_not_empty() {
            panic!(
                "Failed to log {} message to the follwing loggers: {:?}",
                record.level(),
                error_to_panic
            )
        }
    }

    fn flush(&self) {
        let mut error_to_panic = StaticVec::<&'a str, N>::new();
        let mut error_to_log = StaticVec::<&'a str, N>::new();

        for logger in self.loggers.read().iter() {
            if logger.flush().is_err() {
                if logger.panic_on_flush_error {
                    error_to_panic.push(logger.name);
                }
                error_to_log.push(logger.name);
            }
        }

        if error_to_log.is_not_empty() {
            self.log_log_error("flush", &error_to_log);
        }

        if error_to_panic.is_not_empty() {
            panic!("Failed to flush the follwing loggers: {:?}", error_to_panic)
        }
    }
}

macro_rules! record {
    ($lvl:expr, $($arg:tt)+) => ({
        let lvl = $lvl;
        let target = module_path!();
        let file = file!();
        let line = line!();

        &Record::builder()
            .args(format_args!($($arg)+))
            .level(lvl)
            .target(target)
            .file(Some(file))
            .line(Some(line))
            .module_path(Some(target))
            .build()
    });
}

impl<'a, const N: usize, const L: usize, I: InterruptState> DispatchLogger<'a, I, N, L> {
    fn log_log_error<D: Display>(&self, error_reason: D, failing_loggers: &[&'a str]) {
        for primary in self.loggers.read().iter().filter(|l| l.primary) {
            if primary
                .log(record!(Level::Error, "Failed to {}", error_reason))
                .is_err()
            {
                break;
            }
            for &failed in failing_loggers {
                if primary
                    .log(record!(
                        Level::Warn,
                        "Logger {} failed to {}",
                        failed,
                        error_reason
                    ))
                    .is_err()
                {
                    break;
                }
            }
        }
    }
}
