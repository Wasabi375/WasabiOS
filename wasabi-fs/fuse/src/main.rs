use std::path::PathBuf;

use clap::Parser;
use fuse::WasabiFuseTest;
use fuser::MountOption;
use log::{debug, LevelFilter};
use simple_logger::SimpleLogger;

mod fuse;

/// The FUSE driver for the Wasabi Filesystem
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Arguments {
    /// mount point for the fs
    mount_point: PathBuf,

    /// fs image file
    image_file: PathBuf,

    /// Allow all users to access files on this filesystem. By default access is restricted to the
    /// user who mounted it
    #[arg(long)]
    allow_other: bool,

    /// verbose logging
    #[arg(short, long)]
    verbose: bool,
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
