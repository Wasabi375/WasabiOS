use clap::ValueEnum;
use congen::Configuration;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    config::{
        build::{BuildArtifact, BuildTarget},
        general::SizeBytes,
    },
    config_id_type,
};

config_id_type!(FsId, FsIdT);
config_id_type!(ImageId, ImageIdT);

#[derive(Debug, Clone, Configuration, Serialize, Deserialize)]
#[congen(debug, clone)]
pub struct DiskImage {
    pub id: ImageId,
    pub partitions: Vec<Partition>,
    pub device_uuid: Option<String>,
}

impl DiskImage {
    pub fn device_uuid(&self) -> Option<Uuid> {
        match self.device_uuid.as_ref() {
            Some(uuid) => Some(Uuid::parse_str(uuid).unwrap()),
            None => None,
        }
    }
}

#[derive(Debug, Clone, Configuration, Serialize, Deserialize)]
#[congen(debug, clone)]
pub struct Partition {
    #[congen(rust_default)]
    pub uuid: Option<String>,
    #[congen(rust_default)]
    pub name: String,
    #[congen(default = FsId::new(""))]
    pub fs: FsId,
    #[congen(default = false)]
    pub is_boot: bool,
    #[congen(default = false)]
    pub is_read_only: bool,
}

impl Partition {
    pub fn uuid(&self) -> Option<Uuid> {
        match self.uuid.as_ref() {
            Some(uuid) => Some(Uuid::parse_str(uuid).unwrap()),
            None => None,
        }
    }
}

#[derive(Debug, Clone, Configuration, Serialize, Deserialize)]
#[congen(debug, clone)]
pub struct FileSystem {
    #[congen(rust_default)]
    pub id: FsId,

    #[congen(default)]
    pub fs_type: FsType,

    /// Size of the filesytem
    ///
    /// not all file system types respect this.
    pub size: SizeBytes,

    /// Data directory that is copied into the fs
    #[congen(rust_default)]
    pub static_input: Option<String>,

    pub generated: Vec<GeneratedFsInputConfig>,
}

#[derive(Debug, Clone, Configuration, Serialize, Deserialize)]
#[congen(debug, clone)]
pub struct GeneratedFsInputConfig {
    #[congen(rust_default)]
    pub path_in_fs: String,

    #[congen(rust_default)]
    pub build: BuildTarget,

    #[congen(default = BuildArtifact::KernelElf)]
    pub artifact: BuildArtifact,
}

#[derive(
    Debug,
    Default,
    Copy,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Configuration,
    ValueEnum,
    Serialize,
    Deserialize,
)]
#[congen(debug, clone)]
pub enum FsType {
    #[congen(default)]
    #[default]
    WFS,
    FAT,
}
