#![feature(new_zeroed_alloc)]
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use fuse::WasabiFuseTest;
use fuser::MountOption;
use log::{debug, LevelFilter};
use simple_logger::SimpleLogger;
use wfs::{
    blocks_required_for,
    fs::{FileSystem, FsReadWrite, OverrideCheck},
};

use crate::block_device::FileDevice;

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
    Mount(MountOptions),
    Create(CreateOptions),
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
    }
}

fn create(args: CreateOptions) {
    let size_in_bytes = args.size * 1024u64.pow(args.size_magnitude);
    let block_count = blocks_required_for!(size_in_bytes);
    let size_in_bytes = block_count * 512;

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
    let name = args.name.as_ref().map(|n| n.as_str());

    let device = FileDevice::open(&args.image_file, block_count);

    let fs = FileSystem::<_, FsReadWrite>::create(device, override_check, uuid, name).unwrap();

    fs.close().unwrap().close();
}

fn mount(args: MountOptions) {
    let mut options = vec![
        MountOption::AutoUnmount,
        MountOption::AllowRoot,
        MountOption::RO,
        MountOption::FSName("WasabiFS".to_string()),
    ];

    if args.allow_other {
        debug!("allow other");
        options.push(MountOption::AllowOther);
    }

    fuser::mount2(WasabiFuseTest, &args.mount_point, &options).expect("Wasabi Fuse Driver failed");
}
