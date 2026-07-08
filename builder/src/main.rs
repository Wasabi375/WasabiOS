//! Runner and Build tool for Wasabi Os
//!
//! This simple command line utility can be used to run WasabiOs in Qemu.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Parser;
use congen::Configuration;
use log::{debug, warn};
use simple_logger::SimpleLogger;
use tokio::fs;

use crate::{
    args::{Arguments, CleanArgs, Command},
    cachedir::{is_cachedir_file, mark_dir_cachedir},
    config::{Config, VerifiedConfig, init_config},
};

mod args;
mod build;
mod cachedir;
mod cargo;
mod config;
mod disk_image;
mod file_system;
mod ovmf;
mod run;
mod sync;
mod tests;

#[cfg(unix)]
mod unix;

fn verify_workdir() {
    fn check_dir(dir: &Path) -> bool {
        assert!(dir.is_dir());

        let jj_exists = dir.join(".jj").exists();
        let cargo_exists = dir.join("Cargo.toml").exists();

        jj_exists && cargo_exists
    }
    let current_dir = std::env::current_dir().unwrap();
    if !check_dir(&current_dir) {
        warn!("Not in project root. Trying parent once...");
        let parent = current_dir.parent().unwrap();
        assert!(check_dir(parent));
        warn!(
            "parent looks like project root. Set working dir to {}",
            parent.display()
        );
        std::env::set_current_dir(parent).unwrap();
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    #[cfg(unix)]
    let _restore_tty = unix::RestoreTty::new()?;

    SimpleLogger::new()
        .with_level(log::LevelFilter::Info)
        .env()
        .init()
        .unwrap();

    verify_workdir();

    let args = Arguments::parse();

    if let Command::InitConfig { reset } = &args.cmd {
        init_config(&args.config, *reset)?;
        return Ok(());
    };

    let config = Config::load(&args.config)?;

    if let Command::Config(change) = &args.cmd {
        config
            .with_change(change.clone().into_change())
            .write_update(&args.config)?;
        return Ok(());
    }

    let env_changes =
        congen::env::load_from_env::<Config>("WOS").context("load config changes from env")?;
    let out_dir: PathBuf = config.out_dir.clone().into();

    let config = config
        .with_change(env_changes)
        .verify()
        .context("verify config")?;

    match args.cmd {
        Command::Build(args) => build::build_from_args(args, config)
            .await
            .context("build")?,
        Command::Check(args) => build::check(args, config).await.context("check")?,
        Command::BuildImage(args) => disk_image::build_from_args(args, config)
            .await
            .context("build image")?,
        Command::Clean(args) => clean(args, config).await.context("clean")?,
        Command::Run(args) => run::run_with_args(args, config).await.context("run")?,
        Command::Test(args) => tests::test_with_args(args, config).await.context("test")?,
        Command::Gdb => todo!(),
        Command::Config(_) | Command::InitConfig { .. } => {
            unreachable!("Handled before loading config")
        }
        Command::VerifyConfig => {
            // No-op. We have a verified config. This is here to allow for a no-op cmd to check
            let _: VerifiedConfig = config;
        }
        Command::DownloadOvmf => ovmf::download(&config.config)
            .await
            .context("download-ovmf")?,
        Command::Fs(args) => file_system::build_from_args(args, config)
            .await
            .context("fs build")?,
    }

    if out_dir.exists() {
        mark_dir_cachedir(out_dir, true)
            .await
            .context("mark outdir as cachedir")?;
    }

    Ok(())
}

async fn clean(clean: CleanArgs, config: VerifiedConfig) -> Result<()> {
    let out_dir: PathBuf = config.config.out_dir.clone().into();

    if clean.force || clean.force_out {
        warn!("force delete out dir");
        if out_dir.exists() {
            fs::remove_dir_all(out_dir)
                .await
                .context("remove out dir")?;
        }
    }
    if clean.force {
        warn!("force delete target dir");
        let target_dir: PathBuf = std::env::var("CARGO_BUILD_TARGET_DIR")
            .or(std::env::var("CARGO_TARGET_DIR"))
            .unwrap_or("target".to_string())
            .into();

        if target_dir.exists() {
            let target_dir = target_dir
                .canonicalize()
                .context("get absolute target dir")?;
            let current_dir = std::env::current_dir()
                .context("get current dir")?
                .canonicalize()
                .context("get absolute current dir")?;
            if target_dir.starts_with(&current_dir) {
                fs::remove_dir_all(target_dir)
                    .await
                    .context("remove rust target dir")?;
            } else {
                warn!(
                    "cargo target-dir is not relative to current dir and will not be delted. Run `cargo clean` instead"
                );
            }
        }
        return Ok(());
    }

    ovmf::clean(&clean, &config.config)
        .await
        .context("ovmf::clean")?;
    build::clean(&clean, &config)
        .await
        .context("build::clean")?;
    file_system::clean(&clean, &config.config)
        .await
        .context("file_system::clean")?;
    disk_image::clean(&clean, &config.config)
        .await
        .context("disk_image::clean")?;

    delete_empty_dirs(&config.config.out_dir)
        .await
        .context("delete empty out_dir subdirectories")?;

    Ok(())
}

fn delete_empty_dirs<P>(start_path: &P) -> impl Future<Output = Result<()>> + Send + Sync
where
    P: AsRef<Path> + Send + Sync,
{
    async fn is_empty(path: &Path) -> Result<bool> {
        assert!(path.exists() && path.is_dir());
        let mut dir_iter = fs::read_dir(path).await.context("read dir")?;
        while let Some(entry) = dir_iter.next_entry().await.context("get next entry")? {
            if is_cachedir_file(&entry.path())? {
                continue;
            }
            return Ok(false);
        }
        return Ok(true);
    }

    async move {
        debug!("remove empty out-dir subirectories");
        let start_path = start_path.as_ref();

        let mut dir_iter = fs::read_dir(start_path).await.context("read dir")?;
        let mut handles = Vec::new();
        while let Some(entry) = dir_iter.next_entry().await.context("get next entry")? {
            if !entry.file_type().await?.is_dir() {
                continue;
            }
            handles.push(tokio::spawn(async move {
                delete_empty_dirs(&entry.path()).await
            }));
        }
        for handle in handles {
            handle.await??
        }
        if is_empty(start_path).await? {
            fs::remove_dir_all(start_path)
                .await
                .context("remove empty dir")?;
            return Ok(());
        }
        Ok(())
    }
}
