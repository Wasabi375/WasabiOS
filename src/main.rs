//! Runner and Build tool for Wasabi Os
//!
//! This simple command line utility can be used to run WasabiOs in Qemu.
#![feature(exit_status_error)]

mod args;
mod build;
mod qemu;
mod test;

use anyhow::{Context, Result};
use args::{Arguments, BuildCommand, GdbArgs, LatestArgs, Profile, RunArgs, RunCommand, Target};
use build::{build, check, clean, expand};
use clap::Parser;
use log::LevelFilter;
use nix::sys::termios::{tcgetattr, tcsetattr, SetArg, Termios};
use qemu::{launch_qemu, Kernel, QemuConfig};
use simple_logger::SimpleLogger;
use std::{
    ffi::{OsStr, OsString},
    io::Write,
    os::fd::AsFd,
    path::{Path, PathBuf},
    str::FromStr,
};
use test::test;
use tokio::process::Command;

#[tokio::main]
async fn main() -> Result<()> {
    #[cfg(unix)]
    let _restore_tty = RestoreTty::new();

    SimpleLogger::new()
        .with_level(LevelFilter::Info)
        .with_module_level("gpt", LevelFilter::Warn)
        .with_module_level("fatfs", LevelFilter::Warn)
        .env()
        .init()
        .unwrap();
    let args = Arguments::parse();

    match args.build {
        BuildCommand::Build(mut args) => {
            if args
                .run
                .as_ref()
                .map(|run_command| run_command.gdb_enabled())
                .unwrap_or(false)
            {
                args.options.features.push(args::Feature::FixedKernelVaddr);
            }
            build(args).await?
        }
        BuildCommand::Latest(args) => latests(args).await?,
        BuildCommand::Clean(args) => clean(args).await?,
        BuildCommand::Check(args) => check(args).await?,
        BuildCommand::Expand(args) => expand(args).await?,
        BuildCommand::Gdb(args) => gdb(args).await?,
    }

    Ok(())
}

/// the storage location, containing the last successful build of the os
pub fn latest_path(bin_name: &OsStr, target: &Target, profile: &Profile) -> PathBuf {
    let mut path = PathBuf::new();
    path.push("latest");
    path.push(target.tripple_str());
    path.push(profile.as_os_str());
    path.push(bin_name);

    path
}

/// runs the kernel in qemu
pub async fn run(kernel_path: &Path, args: RunArgs) -> Result<()> {
    let kernel = Kernel { path: &kernel_path };

    let qemu = QemuConfig::from_options(&args.qemu);

    let mut process = launch_qemu(&kernel, &qemu).await.context("start kernel")?;
    process.wait().await.context("waiting on qemu")?;

    Ok(())
}

/// execute [BuildCommand::Latest]
pub async fn latests(args: LatestArgs) -> Result<()> {
    let mut bin_name = OsString::from_str(match args.run {
        RunCommand::Run(_) => "wasabi-kernel",
        RunCommand::Test(_) => "wasabi-test",
    })
    .unwrap();

    let mut path = latest_path(&bin_name, &args.target, &args.profile);

    bin_name.push("-uefi.img");
    path.push(&bin_name);
    match args.run {
        RunCommand::Run(args) => run(&path, args).await,
        RunCommand::Test(args) => test(&path, args).await,
    }
}

pub async fn gdb(args: GdbArgs) -> Result<()> {
    let bin_name = args.bin.name();
    let mut path = latest_path(bin_name, &args.target, &args.profile.unwrap_or_default());
    path.push(bin_name);

    let gdb_commands = [
        format!("target remote localhost:{}", args.port.unwrap_or(1234)),
        // address is found in wasabi_kernel/src/lib.rs: KERNEL_BINARY_VADDR
        format!("symbol-file -o 0xff000000000 {}", path.display()),
        format!("break wasabi_kernel::kernel_bsp_entry"),
    ]
    .into_iter()
    .map(|c| format!("--eval-command={}", c));

    if args.print_command {
        print_gdb_command(args, path.to_str().unwrap(), gdb_commands);
        return Ok(());
    }
    let mut cmd = Command::new("rust-gdb");
    cmd.arg(path);

    cmd.args(gdb_commands);

    log::info!("{cmd:?}");
    cmd.spawn()
        .context("gdb")?
        .wait()
        .await
        .context("waiting for gdb")?;
    Ok(())
}

fn print_gdb_command(_args: GdbArgs, bin_name: &str, commands: impl Iterator<Item = String>) {
    let mut cmd = String::new();
    cmd.push_str("rust-gdb ");
    cmd.push_str(bin_name);
    for c in commands {
        cmd.push(' ');
        cmd.push_str(&c);
    }
    println!("{}", cmd);
}

#[cfg(unix)]
/// Takes a snapshot of the tty settings and restores them on drop.
///
/// For some reasons QEMU sometimes mangles the tty. This used to be
/// pretty common during tests.
/// This makes sure the tty is useable after qemu is finished
struct RestoreTty {
    stdout: Termios,
    stderr: Termios,
}

#[cfg(unix)]
impl RestoreTty {
    fn new() -> Result<Self> {
        let stdout =
            tcgetattr(std::io::stdout().as_fd()).context("Failed to read Termios for stdout")?;
        let stderr =
            tcgetattr(std::io::stderr().as_fd()).context("Failed to read Termios for stderr")?;

        Ok(Self { stdout, stderr })
    }
}

#[cfg(unix)]
impl Drop for RestoreTty {
    fn drop(&mut self) {
        let mut stdout = std::io::stdout();
        let mut stderr = std::io::stderr();
        let _ = stdout.flush();
        let _ = stderr.flush();
        let res_1 = tcsetattr(stdout.as_fd(), SetArg::TCSANOW, &self.stdout)
            .context("Failed to restore Termios for stdout");
        let res_2 = tcsetattr(stderr.as_fd(), SetArg::TCSANOW, &self.stderr)
            .context("Failed to restore Termios for stderr");

        match (res_1.err(), res_2.err()) {
            (Some(e1), Some(e2)) => panic!("Failed to restore Termios:\n{e1}\n{e2}"),
            (Some(e), None) | (None, Some(e)) => panic!("Failed to restore Termios:\n{e}"),
            _ => {}
        }
    }
}
