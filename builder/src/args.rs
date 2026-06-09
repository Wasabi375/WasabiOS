use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, value_parser};
use congen::CongenClap;

use crate::config::{
    Config,
    build::BuildTarget,
    file_system::{FsId, ImageId},
};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Arguments {
    #[command(subcommand)]
    pub cmd: Command,

    #[arg(long, short, default_value = Config::DEFAULT_PATH, value_parser = value_parser!(PathBuf))]
    pub config: PathBuf,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    #[command(alias = "b")]
    Build(BuildArgs),
    BuildImage(BuildImageArgs),
    Clean(CleanArgs),
    #[command(alias = "c")]
    Check(CheckArgs),
    #[command(alias = "r")]
    Run(RunArgs),
    #[command(alias = "t")]
    Test,
    Gdb,

    Fs(FsArgs),

    DownloadOvmf,

    /// Update the config file
    Config(CongenClap<Config>),
    /// Create a new "empty" config
    InitConfig {
        /// reset the config
        #[arg(long)]
        reset: bool,
    },
    VerifyConfig,
}

#[derive(Args, Debug)]
pub struct BuildArgs {
    #[arg(long)]
    pub target: Vec<BuildTarget>,
}

#[derive(Args, Debug)]
pub struct CheckArgs {
    #[arg(long)]
    pub target: Vec<BuildTarget>,
}

#[derive(Args, Debug)]
pub struct BuildImageArgs {
    #[arg(long)]
    pub target: Vec<ImageId>,
}

#[derive(Args, Debug)]
pub struct CleanArgs {
    /// override any config to keep data
    #[arg(long)]
    pub all: bool,

    /// force delete out and target directory
    #[arg(long)]
    pub force: bool,

    /// force delete just the out directory
    #[arg(long)]
    pub force_out: bool,
}

#[derive(Args, Debug)]
pub struct FsArgs {
    pub target: FsId,
}

#[derive(Args, Debug)]
pub struct RunArgs {
    pub target: Option<ImageId>,
}
