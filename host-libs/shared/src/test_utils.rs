//! Utils for tests run on the host system

use std::sync::Once;

static LOGGER_INIT: Once = Once::new();

/// Initializes a logger
///
/// This can be called multiple times and ensures that
/// the logger is only initialized once.
///
/// This is automatically called for multitest (see testing/derive crate)
pub fn init_test_logger() {
    #[cfg(not(miri))]
    LOGGER_INIT.call_once(|| {
        env_logger::init();
    });
    #[cfg(miri)]
    LOGGER_INIT.call_once(|| {
        use log::LevelFilter;
        use std::str::FromStr;

        // miri isolates the program from the environment. We use the compile
        // time variable to get around this.
        // This should be fine as we never use miri to just generate binaries
        let level = if let Some(level_name) = std::option_env!("RUST_LOG") {
            LevelFilter::from_str(&level_name).unwrap_or(LevelFilter::Info)
        } else {
            LevelFilter::Info
        };
        println!("level: {:?}", level);
        env_logger::builder()
            .filter_level(level)
            .format_timestamp(None)
            .init();
    })
}
