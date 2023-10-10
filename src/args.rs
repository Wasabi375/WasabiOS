use clap::{Args, Parser, Subcommand, ValueEnum};
use std::ffi::OsStr;

/// Runner for WasabiOs
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Arguments {
    #[command(subcommand)]
    pub build: BuildCommand,
}

#[derive(Debug, Subcommand)]
pub enum BuildCommand {
    /// rebuild the kernel
    Build(BuildArgs),
    /// use the latest available build. Fails if no build exists
    Latest(LatestArgs),
    /// Cleanup build artifacts
    Clean(CleanArgs),
    /// run cargo check on kernel
    Check(CheckArgs),
    // TODO Docs command for building and opening docs
}

#[derive(Debug, Subcommand)]
pub enum RunCommand {
    /// run the kernel in qemu
    Run,
    /// run kernel tests in qemu
    Test(TestArgs),
}

#[derive(Args, Debug)]
pub struct TestArgs {
    /// fails qemu instance after `timeout` seconds.
    ///
    /// Depending on the settings, qemu might be started multiple times to finishe testing
    #[arg(long, default_value_t = 10)]
    pub timeout: u64,

    /// run each test in it's own instance of qemu
    ///
    /// This is can be usefull to isolate global state change introduced by one test
    /// that might cause other tests to fail, but will drastically increase the time
    /// it takes to finish testing, because qemu combined with the bootloader and kernel
    /// startup add up to a not insignificant amount of time.
    #[arg(long, short)]
    pub isolated: bool,

    /// continue testing even if a test panics, by restarting qemu.
    #[arg(long)]
    pub keep_going: bool,

    /// ignore tests that are expected to panic
    ///
    /// those tests each require a new instance of qemu and are therefor quite slow,
    #[arg(long, short)]
    pub fast: bool,

    /// the tcp port used to communicate with the kernel.
    ///
    /// this needs to be an open port. Can be set to "none", which will disable "keep_going"
    /// and "isolated" and will set "fast" as those features rely on the communication
    /// through the tcp port.
    #[arg(short, long, default_value_t = String::from("4444"))]
    pub tcp_port: String,
}

#[derive(Args, Debug)]
pub struct BuildArgs {
    #[command(flatten)]
    pub options: BuildOptions,

    /// skip building tests, this cannot be used with the "test" command
    #[arg(long)]
    pub no_tests: bool,

    #[arg(long)]
    pub emit_asm: bool,

    #[command(subcommand)]
    pub run: Option<RunCommand>,
}

#[derive(Args, Debug)]
pub struct LatestArgs {
    #[arg(short, long)]
    pub profile: Profile,

    /// target architecture
    #[arg(long, default_value = Target::X86_64)]
    pub target: Target,

    #[command(subcommand)]
    pub run: RunCommand,
}

#[derive(Args, Debug)]
pub struct CleanArgs {
    #[arg(long, short)]
    pub workspace: bool,

    #[arg(long, short)]
    pub kernel: bool,

    #[arg(long, short)]
    pub test: bool,

    #[arg(long, short)]
    pub latest: bool,

    #[arg(long, short)]
    pub all: bool,

    #[arg(long)]
    pub docs: bool,
}

#[derive(Args, Debug)]
pub struct CheckArgs {
    #[command(flatten)]
    pub options: BuildOptions,

    /// skip building tests, this cannot be used with the "test" command
    #[arg(long)]
    pub no_tests: bool,
}

#[derive(Args, Debug)]
pub struct BuildOptions {
    /// build profile
    #[arg(short, long)]
    pub profile: Option<Profile>,

    /// use release profile, overwritten by [profile]
    #[arg(short, long)]
    pub release: bool,

    /// target architecture
    #[arg(long, default_value = Target::X86_64)]
    pub target: Target,

    /// build features
    #[arg(long)]
    pub features: Vec<Feature>,

    /// build with all features, overwrites [features]
    #[arg(long)]
    pub all_features: bool,

    /// disable default features. Features can be reenabled with [features]
    #[arg(long)]
    pub no_default_features: bool,
}

impl BuildOptions {
    pub fn profile(&self) -> Profile {
        self.profile.unwrap_or_else(|| {
            if self.release {
                Profile::Release
            } else {
                Profile::Dev
            }
        })
    }
}

/// Build the OS with the specified Profile (default debug)
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum Profile {
    /// Dev profile (default)
    Dev,
    /// Release profile
    Release,
}

impl Profile {
    pub fn as_os_str(&self) -> &'static OsStr {
        match self {
            Profile::Dev => OsStr::new("dev"),
            Profile::Release => OsStr::new("release"),
        }
    }
}

/// Target architecture (default X86_64)
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum Target {
    /// X86-64
    X86_64,
}

impl Into<clap::builder::OsStr> for Target {
    fn into(self) -> clap::builder::OsStr {
        match self {
            Target::X86_64 => "x86-64".into(),
        }
    }
}

impl Target {
    pub fn tripple_str(&self) -> &'static OsStr {
        match self {
            Target::X86_64 => OsStr::new("x86_64-unknown-none"),
        }
    }
}

/// Build feature used for `cfg`
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum Feature {
    NoUnicodeLog,
    NoColor,
}

impl Feature {
    pub fn as_os_str(&self) -> &'static OsStr {
        match self {
            Feature::NoUnicodeLog => OsStr::new("no-unicode-log"),
            Feature::NoColor => OsStr::new("no-color"),
        }
    }
}
