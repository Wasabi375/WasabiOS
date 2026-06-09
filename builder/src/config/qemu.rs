use congen::Configuration;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Configuration, Serialize, Deserialize)]
#[congen(debug, clone)]
pub struct QemuConfig {
    pub ovmf: Ovmf,

    pub memory: String,
    pub processor_count: u8,

    pub serial: Vec<String>,

    /// file path for qemu debug log
    pub debug_log: Option<String>,
    /// qemu debug info argument
    pub debug_info: String,
}

#[derive(Debug, Clone, Configuration, Serialize, Deserialize)]
#[congen(debug, clone)]
pub struct Ovmf {
    pub prebuild_tag: String,
    pub prebuild_hash: String,

    pub storage_path: String,
    pub download_url: String,
}
