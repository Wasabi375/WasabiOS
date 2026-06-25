use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    sync::Arc,
};

use clap::ValueEnum;
use congen::Configuration;
use serde::{Deserialize, Serialize};

use crate::{config::Config, config_id_type};

config_id_type!(KernelId, KernelIdT);
config_id_type!(BuildTarget, BuildTargetT);

impl BuildTarget {
    pub const BOOTLOADER_X86: &str = "BOOTLOADER-X86_64";
    pub const RESERVED: &[&str] = &[BuildTarget::BOOTLOADER_X86];
}

impl From<KernelId> for BuildTarget {
    fn from(value: KernelId) -> Self {
        value.into_inner().into()
    }
}

#[derive(Debug, Clone, Configuration, Serialize, Deserialize)]
#[congen(debug, clone)]
pub struct GeneralBuild {
    pub jobs: Option<u8>,

    pub architecture: Arch,
}

#[derive(Debug, Clone)]
pub enum BuildConfig {
    Kernel(Arc<KernelBuild>),
    /// Hardcoded BuildTargets that require special build logic, e.g bootloader
    Special(BuildTarget),
}

impl BuildConfig {
    pub fn id(&self) -> BuildTarget {
        match self {
            BuildConfig::Kernel(kernel_build) => kernel_build.id.clone().into(),
            BuildConfig::Special(id) => id.clone(),
        }
    }

    pub fn out_dir(&self, base_dir: impl AsRef<Path>, general_build: &GeneralBuild) -> PathBuf {
        match self {
            BuildConfig::Kernel(kernel_build) => {
                crate::build::out_dir_for(base_dir, &kernel_build, general_build)
            }
            BuildConfig::Special(id) => match id.as_str() {
                BuildTarget::BOOTLOADER_X86 => crate::build::bootloader_out_dir(base_dir, id),
                _ => panic!("unknown special id: {id}"),
            },
        }
    }

    pub fn artifact_path(&self, artifact: BuildArtifact, config: &Config) -> PathBuf {
        let dir = self.out_dir(&config.out_dir, &config.general_build);
        let name = self.artifact_name(artifact);
        dir.join(name)
    }

    /// path relative to [Self::out_dir] for the specific artifact
    pub fn artifact_name(&self, artifact: BuildArtifact) -> String {
        match self {
            BuildConfig::Kernel(kernel_build) => {
                crate::build::artifact_path(&kernel_build, artifact)
            }
            BuildConfig::Special(id) => match id.as_str() {
                BuildTarget::BOOTLOADER_X86 => crate::build::bootloader_artifact_path(artifact, id),
                _ => panic!("unknown special id: {id}"),
            },
        }
    }

    pub fn specials() -> Vec<(BuildTarget, BuildConfig)> {
        [BuildTarget::BOOTLOADER_X86.into()]
            .into_iter()
            .map(|t: BuildTarget| (t.clone(), BuildConfig::Special(t)))
            .collect()
    }
}

#[derive(Debug, Clone, Configuration, Serialize, Deserialize)]
#[congen(debug, clone)]
pub struct KernelBuild {
    #[congen(rust_default)]
    pub id: KernelId,

    #[congen(rust_default)]
    pub crate_path: String,

    #[congen(default = String::new())]
    pub package_name: String,

    pub features: Vec<String>,

    #[congen(default = false)]
    pub no_default_feature: bool,

    #[congen(default = false)]
    pub all_features: bool,

    pub profile: String,

    #[congen(default = false)]
    pub output_asm: bool,
}

#[derive(Debug, Clone, Configuration, Serialize, Deserialize)]
#[congen(debug, clone)]
pub struct SpecialBuilds {
    pub bootloader: BootloaderBuild,
}

#[derive(Debug, Clone, Configuration, Serialize, Deserialize)]
#[congen(debug, clone)]
pub struct BootloaderBuild {
    pub force_reinstall: bool,
}

/// Target architecture (default X86_64)
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
pub enum Arch {
    /// X86-64
    #[congen(default)]
    #[default]
    X86_64,
}

impl Into<clap::builder::OsStr> for Arch {
    fn into(self) -> clap::builder::OsStr {
        match self {
            Arch::X86_64 => "x86-64".into(),
        }
    }
}

impl Arch {
    pub fn tripple_str(&self) -> &'static OsStr {
        match self {
            Arch::X86_64 => OsStr::new("x86_64-unknown-none"),
        }
    }
    pub fn short_name(&self) -> &'static str {
        match self {
            Arch::X86_64 => "x64",
        }
    }
    pub fn qemu_extension(&self) -> &'static str {
        match self {
            Arch::X86_64 => "-system-x86_64",
        }
    }
}

#[derive(
    Debug,
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
pub enum BuildArtifact {
    /// A standalone kernel bootable using the bootloader, or the bootloader itself
    KernelElf,
    /// A module that can be loaded by the kernel
    KernelModule,
    /// A user mode program
    Executable,
    /// A user mode library
    Lib,
    /// The decompiled asm of the build
    ///
    /// There is no guarantee that it is possible to assemble this into an executable.
    /// This is more for debug purposes.
    Asm,
}
