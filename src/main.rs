//! Runner and Build tool for Wasabi Os
//!
//! This simple command line utility can be used to run WasabiOs in Qemu.
#![feature(exit_status_error)]

mod args;
mod build;
mod qemu;
mod test;

use anyhow::{Context, Result};
use args::{Arguments, BuildCommand, LatestArgs, Profile, RunCommand, Target};
use build::{build, check, clean};
use clap::Parser;
use log::LevelFilter;
use qemu::{launch_qemu, Kernel, QemuConfig};
use simple_logger::SimpleLogger;
use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    str::FromStr,
};
use test::test;

#[tokio::main]
async fn main() -> Result<()> {
    SimpleLogger::new()
        .with_level(LevelFilter::Info)
        .with_module_level("gpt", LevelFilter::Warn)
        .with_module_level("fatfs", LevelFilter::Warn)
        .env()
        .init()
        .unwrap();
    let args = Arguments::parse();

    match args.build {
        BuildCommand::Build(args) => build(args).await,
        BuildCommand::Latest(args) => latests(args).await,
        BuildCommand::Clean(args) => clean(args).await,
        BuildCommand::Check(args) => check(args).await,
    }
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
pub async fn run(uefi: &Path) -> Result<()> {
    let kernel = Kernel {
        path: &uefi,
        uefi: true,
    };

    let qemu = QemuConfig::default();

    launch_qemu(&kernel, &qemu)
        .await
        .context("start kernel")?
        .wait()
        .await
        .context("waiting on qemu")?;

    Ok(())
}

/// execute [BuildCommand::Latest]
pub async fn latests(args: LatestArgs) -> Result<()> {
    let mut bin_name = OsString::from_str(match args.run {
        RunCommand::Run => "wasabi-kernel",
        RunCommand::Test(_) => "wasabi-test",
    })
    .unwrap();

    let mut path = latest_path(&bin_name, &args.target, &args.profile);

    bin_name.push("_uefi.img");
    path.push(&bin_name);
    match args.run {
        RunCommand::Run => run(&path).await,
        RunCommand::Test(args) => test(&path, args).await,
    }
}
