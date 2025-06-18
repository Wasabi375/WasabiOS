#![feature(new_zeroed_alloc, allocator_api)]
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use fuser::MountOption;
use log::{LevelFilter, debug, info};
use simple_logger::SimpleLogger;
use wfs::{
    BLOCK_SIZE, blocks_required_for,
    fs::{FsReadOnly, FsReadWrite, OverrideCheck},
};

use crate::fuse::WasabiFuse;

mod fuse;

mod block_device;

/// The FUSE driver for the Wasabi Filesystem
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Arguments {
    /// verbose logging
    #[arg(short, long)]
    verbose: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Mount the image at the given mount point
    ///
    /// If this command is chosen, the process must not be interrupted using Ctrl+C.
    /// Instead use the `umount` program to unmount the fs instad
    Mount(MountOptions),
    Create(CreateOptions),
    Info(InfoOptions),
    ResetTransient(ForceResetOptions),
}

#[derive(Args, Debug)]
struct InfoOptions {
    image_file: PathBuf,
}

#[derive(Args, Debug)]
struct ForceResetOptions {
    image_file: PathBuf,
}

#[derive(Args, Debug)]
struct CreateOptions {
    /// the size of the file system
    size: u64,

    /// the magnitude of the file sytem size
    ///
    /// 0: Bytes
    /// 1: KiloBytes
    /// 2: MegaBytes
    /// 3: GigaBytes
    /// ...
    size_magnitude: u32,

    /// fs image file
    image_file: PathBuf,

    #[arg(long)]
    name: Option<String>,

    /// override any existing file
    #[arg(long, short)]
    force: bool,
}

#[derive(Args, Debug)]
struct MountOptions {
    /// mount point for the fs
    mount_point: PathBuf,

    /// fs image file
    image_file: PathBuf,

    /// Allow all users to access files on this filesystem. By default access is restricted to the
    /// user who mounted it
    #[arg(long)]
    allow_other: bool,
}

fn main() {
    let args = Arguments::parse();
    let log_level = if args.verbose {
        if cfg!(debug_assertions) {
            LevelFilter::Trace
        } else {
            LevelFilter::Debug
        }
    } else {
        LevelFilter::Info
    };
    SimpleLogger::new()
        .with_level(log_level)
        .with_module_level("fuser", LevelFilter::Info)
        .env()
        .init()
        .unwrap();

    match args.command {
        Command::Mount(args) => mount(args),
        Command::Create(args) => create(args),
        Command::Info(args) => info(args),
        Command::ResetTransient(args) => reset_transient(args),
    }
}

fn reset_transient(args: ForceResetOptions) {
    WasabiFuse::<FsReadWrite>::force_open(&args.image_file).unwrap();
}

fn info(args: InfoOptions) {
    let fs = WasabiFuse::<FsReadOnly>::open(&args.image_file).unwrap();
    info!("Header: {:?}", fs.fs().header_data());
}

fn create(mut args: CreateOptions) {
    let size_in_bytes = args.size * 1024u64.pow(args.size_magnitude);
    let block_count = blocks_required_for!(size_in_bytes);
    let size_in_bytes = block_count * BLOCK_SIZE as u64;

    println!(
        "Create filesystem with {} bytes / {} blocks",
        size_in_bytes, block_count
    );

    let override_check = if args.force {
        OverrideCheck::IgnoreExisting
    } else {
        OverrideCheck::Check
    };
    let uuid = uuid::Uuid::new_v4();
    let name: Option<Box<str>> = args.name.take().map(|n| n.into_boxed_str());

    WasabiFuse::create(&args.image_file, block_count, override_check, uuid, name).unwrap();
}

fn mount(args: MountOptions) {
    let fs = WasabiFuse::<FsReadWrite>::open(&args.image_file).unwrap();
    let name = fs
        .fs()
        .header_data()
        .name
        .clone()
        .map(|n| n.into())
        .unwrap_or("WasabiFS".to_string());

    let mut options = vec![
        fuser::MountOption::AutoUnmount,
        fuser::MountOption::AllowRoot,
        fuser::MountOption::RW,
        fuser::MountOption::FSName(name),
    ];

    if args.allow_other {
        debug!("allow other");
        options.push(MountOption::AllowOther);
    }

    fuser::mount2(fs, &args.mount_point, &options).expect("Wasabi Fuse Driver failed");
}
