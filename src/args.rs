use clap::{Args, Parser, Subcommand, ValueEnum};
use std::{
    ffi::{OsStr, OsString},
    path::PathBuf,
};

use crate::build::KernelBinary;

/// Runner for WasabiOs
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Arguments {
    #[command(subcommand)]
    pub build: BuildCommand,
}

/// The different build commands
#[derive(Debug, Subcommand)]
pub enum BuildCommand {
    /// rebuild the kernel
    #[command(alias = "b")]
    Build(BuildArgs),
    /// use the latest available build. Fails if no build exists
    Latest(LatestArgs),
    /// Cleanup build artifacts
    Clean(CleanArgs),
    /// run cargo check on kernel
    #[command(alias = "c")]
    Check(CheckArgs),
    /// run cargo expand on kernel
    Expand(ExpandArgs),
    // TODO Docs command for building and opening docs
}

/// The different ways to run the kernel
#[derive(Debug, Subcommand)]
pub enum RunCommand {
    /// run the kernel in qemu
    #[command(alias = "r")]
    Run(RunArgs),
    /// run kernel tests in qemu
    #[command(alias = "t")]
    Test(TestArgs),
}
#[derive(Args, Debug)]
pub struct UefiOptions {
    /// The ovmf code file to use
    #[arg(long)]
    pub ovmf_code: Option<PathBuf>,
    /// The ovmf var file to use
    #[arg(long)]
    pub ovmf_vars: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct QemuOptions {
    /// This enables the qemu log
    ///
    /// the logging options are defined using `qemu-info`
    /// and the log is stored to the given path
    ///
    /// TODO test will run qemu multiple times, we should make log paths somewhat unique
    #[arg(long)]
    pub qemu_log: Option<PathBuf>,

    /// the qemu info to log
    ///
    /// this is only used if `qemu-log` is set
    #[arg(long, default_value = "int,cpu_reset,unimp,guest_errors")]
    pub qemu_info: String,

    /// The number of processors simulated by qemu
    #[arg(short, long, default_value_t = 4)]
    pub processor_count: u8,

    /// Options for setting up uefi
    #[command(flatten)]
    pub uefi: UefiOptions,

    /// Qemu waits for a debugger to connect
    #[arg(long)]
    pub gdb: bool,

    /// Overrite the default port(1234) for a debugger to connect to qemu
    ///
    /// only used when [gdb] is also specified.
    #[arg(long)]
    pub gdb_port: Option<u16>,
}

#[derive(Args, Debug)]
pub struct RunArgs {
    /// the options used for starting qemu
    #[command(flatten)]
    pub qemu: QemuOptions,
}

/// The required arguments to run the test kernel
#[derive(Args, Debug)]
pub struct TestArgs {
    /// the options used for starting qemu
    #[command(flatten)]
    pub qemu: QemuOptions,

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

/// Arguments for [BuildCommand::Build]
#[derive(Args, Debug)]
pub struct BuildArgs {
    /// the options used to build the kernel
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

    #[arg(long)]
    pub ovmf: bool,
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
pub struct ExpandArgs {
    /// Expand only this package's library
    #[arg(long)]
    pub lib: bool,

    /// Expand only the specified binary
    #[arg(long)]
    pub bin: Option<String>,

    #[command(flatten)]
    pub options: BuildOptions,

    #[arg(short, long, value_enum, default_value_t = WorkspacePackage::WasabiKernel)]
    pub package: WorkspacePackage,

    /// Local path to module or other named item to expand, e.g. cpu::gdt
    pub item: Option<String>,
}

#[derive(Args, Debug)]
pub struct BuildOptions {
    /// build profile
    #[arg(long)]
    pub profile: Option<Profile>,

    /// use release profile, overwritten by [profile]
    #[arg(short, long)]
    pub release: bool,

    /// target architecture
    #[arg(long, default_value = Target::X86_64)]
    pub target: Target,

    /// build features
    #[arg(long, short = 'F')]
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

/// A package in the WasabiOs workspace
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
#[allow(non_camel_case_types)]
pub enum WorkspacePackage {
    WasabiKernel,
    WasabiTest,
    SharedDerive,
    Shared,
    Logger,
    Staticvec,
    InterruptFnBuilder,
    Testing,
    Testing_Derive,
}

impl WorkspacePackage {
    pub fn as_os_path(&self) -> OsString {
        let name = format!("{self:?}");
        let mut name = name.chars();

        let mut output = OsString::new();
        {
            let first = name.next().unwrap();
            if first.is_uppercase() {
                output.push(first.to_lowercase().to_string());
            } else {
                output.push(format!("{first}"));
            }
        }
        for c in name {
            match c {
                '_' => output.push("/"),
                _ if c.is_uppercase() => {
                    output.push("-");
                    output.push(c.to_lowercase().to_string());
                }
                _ => output.push(format!("{c}")),
            }
        }
        output
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
    Test,
    TestTests,
    MemBackedGuardPage,
    MemStats,
}

impl Feature {
    pub fn as_os_str(&self) -> &'static OsStr {
        match self {
            Feature::NoUnicodeLog => OsStr::new("no-unicode-log"),
            Feature::NoColor => OsStr::new("no-color"),
            Feature::Test => OsStr::new("test"),
            Feature::TestTests => OsStr::new("test-tests"),
            Feature::MemBackedGuardPage => OsStr::new("mem-backed-guard-page"),
            Feature::MemStats => OsStr::new("mem-stats"),
        }
    }

    pub fn used_in(&self, binary: &KernelBinary) -> bool {
        use Feature::*;
        let valid_flags: &[Feature] = match binary {
            KernelBinary::Wasabi => &[NoUnicodeLog, NoColor, Test, MemBackedGuardPage, MemStats],
            KernelBinary::Test => &[NoColor, TestTests, MemBackedGuardPage, MemStats],
        };
        valid_flags.contains(self)
    }
}
