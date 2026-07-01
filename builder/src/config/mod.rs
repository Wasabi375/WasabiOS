use std::{
    collections::{HashMap},
    fs::File,
    path::Path,
    sync::Arc,
};

use anyhow::{Context, Result, bail, ensure};
use congen::Configuration;
use log::debug;
use ron::{self, ser::PrettyConfig};
use serde::{Deserialize, Serialize};

use crate::config::{
    build::{
        BootloaderBuild, BuildArtifact, BuildConfig, BuildTarget, GeneralBuild, KernelBuild,
        KernelId, SpecialBuilds,
    },
    file_system::{
        DiskImage, FileSystem, FsId, FsType, GeneratedFsInputConfig, ImageId, Partition,
    },
    general::SizeBytes,
    qemu::{Ovmf, QemuConfig},
    tests::{Test, TestGroup, TestId, TestSystem},
};

pub mod build;
pub mod file_system;
pub mod general;
pub mod id;
pub mod qemu;
pub mod tests;

#[derive(Debug, Clone, Configuration, Serialize, Deserialize)]
#[congen(debug, clone)]
pub struct Config {
    #[congen(default = "latest".to_string())]
    pub out_dir: String,

    pub general_build: GeneralBuild,

    pub special_builds: SpecialBuilds,

    pub kernels: Vec<Arc<KernelBuild>>,

    pub file_systems: Vec<Arc<FileSystem>>,

    pub images: Vec<Arc<DiskImage>>,

    pub tests: Vec<Arc<TestSystem>>,
    pub test_groups: Vec<Arc<TestGroup>>,

    pub default_build_targets: Vec<BuildTarget>,

    pub default_tests: Vec<TestId>,

    pub default_run_image: Option<ImageId>,

    pub qemu: QemuConfig,

    pub clean: CleanConfig,
}

#[derive(Debug, Clone)]
pub struct VerifiedConfig {
    pub config: Config,

    pub build_targets: HashMap<BuildTarget, Arc<BuildConfig>>,
    #[expect(unused)]
    pub kernels: HashMap<KernelId, Arc<KernelBuild>>,

    pub images: HashMap<ImageId, Arc<DiskImage>>,

    pub file_systems: HashMap<FsId, Arc<FileSystem>>,

    pub tests: HashMap<TestId, Test>,
    pub test_systems: HashMap<TestId, Arc<TestSystem>>,
}

impl Config {
    /// The Path to the default Config used by the builder
    ///
    /// Path is relative to the workspace root and not this crate
    pub const DEFAULT_PATH: &'static str = "build-conf.ron";

    fn ron_options() -> ron::Options {
        ron::Options::default()
    }

    pub fn write_update(&self, path: &Path) -> Result<()> {
        let config_file = File::create(path)
            .with_context(|| format!("open config file at: {}", path.display()))?;

        Self::ron_options()
            .to_io_writer_pretty(config_file, self, PrettyConfig::default())
            .context("serialize config into ron")
    }

    pub fn load(path: &Path) -> Result<Self> {
        let config_file =
            File::open(path).with_context(|| format!("open config file at: {}", path.display()))?;

        Self::ron_options()
            .from_reader(config_file)
            .context("deserialize config from ron")
    }

    pub fn verify(self) -> Result<VerifiedConfig> {
        let mut kernels = HashMap::with_capacity(self.kernels.len());

        for kernel in self.kernels.iter() {
            if kernels.insert(kernel.id.clone(), kernel.clone()).is_some() {
                bail!(
                    "kernel ids must be unique. Found multiple kernel definitions using {}",
                    kernel.id
                );
            }
        }

        let mut build_targets = HashMap::with_capacity(kernels.len());
        build_targets.extend(kernels.iter().map(|(id, kernel)| {
            (
                id.clone().into(),
                BuildConfig::Kernel(kernel.clone()).into(),
            )
        }));
        build_targets.extend(BuildConfig::specials());

        for target in &self.default_build_targets {
            ensure!(
                build_targets.contains_key(target),
                "unknown build target {target} in default_build_targets"
            );
            ensure!(
                !BuildTarget::RESERVED
                    .iter()
                    .any(|reserved| *reserved == target.as_str()),
                "build target {target} is reserved"
            )
        }

        let mut file_systems = HashMap::with_capacity(self.file_systems.len());
        for fs in &self.file_systems {
            if file_systems.insert(fs.id.clone(), fs.clone()).is_some() {
                bail!(
                    "file system ids must be unique. Found multiple fs definitions using {}",
                    fs.id
                )
            }
        }

        let mut images = HashMap::with_capacity(self.images.len());
        for image in &self.images {
            if images.insert(image.id.clone(), image.clone()).is_some() {
                bail!(
                    "disk image ids must be unique. Found multiple image definitions using {}",
                    image.id
                )
            }
        }
        // TODO check uuids in Partitions and Images

        let mut test_systems = HashMap::new();
        let mut tests = HashMap::new();
        for test in &self.tests {
            if test_systems.insert(test.id.clone(), test.clone()).is_some() {
                bail!(
                    "test ids must be unique. Found multiple test system definitions using {}",
                    test.id
                )
            }
            tests.insert(test.id.clone(), Test::System(test.clone()));
        }
        for group in &self.test_groups {
            let mut group = group.clone();
            if group.systems.is_empty() {
                let mut edit = group.as_ref().clone();
                edit.systems = self
                    .tests
                    .iter()
                    .filter(|test| {
                        if group.cargo_only && !test.cargo.is_some() {
                            false
                        } else if group.kernel_only && !test.kernel.is_some() {
                            false
                        } else {
                            true
                        }
                    })
                    .map(|test| test.id.clone())
                    .collect();
                group = edit.into();
            } else {
                for test in &group.systems {
                    ensure!(
                        test_systems.contains_key(&test),
                        "test group {} refers to test {}, but no such test was found",
                        group.id,
                        test
                    );
                }
            }

            if tests
                .insert(group.id.clone(), Test::Group(group.clone()))
                .is_some()
            {
                bail!(
                    "test ids must be unique. Found multiple test system/group definitions using {}",
                    group.id
                )
            }
        }

        Ok(VerifiedConfig {
            config: self,
            kernels,
            build_targets,
            file_systems,
            images,
            tests,
            test_systems
        })
    }
}

#[derive(Debug, Clone, Configuration, Serialize, Deserialize)]
#[congen(debug, clone)]
pub struct CleanConfig {
    #[congen(rust_default)]
    pub ovmf: CleanOvmf,

    pub build: CleanBuild,
}

#[derive(Debug, Clone, Configuration, Serialize, Deserialize)]
#[congen(debug, clone)]
pub struct CleanBuild {
    pub bootloader: bool,
}

#[derive(Debug, Default, Clone, Copy, Configuration, Serialize, Deserialize)]
#[congen(debug, clone)]
pub enum CleanOvmf {
    None,
    #[default]
    Unused,
    All,
}

pub fn init_config(path: &Path, reset: bool) -> Result<()> {
    if !reset && path.exists() {
        bail!(
            "A config at {} already exists. You can either reset it by using `--reset` or
            specify a different path using `--config CONFIG_PATH`",
            path.display()
        );
    }

    let config = initial_config();
    debug!("inital config: {config:#?}");
    config.write_update(path)?;

    Ok(())
}

fn initial_config() -> Config {
    const KERNEL_FILE_NAME: &str = "kernel-x86_64";
    const UEFI_BOOT_FILENAME: &str = "efi/boot/bootx64.efi";
    Config {
        out_dir: "builder/out_test".to_string(), // FIXME set back to latest
        general_build: GeneralBuild {
            jobs: None,
            architecture: build::Arch::X86_64,
        },
        special_builds: SpecialBuilds {
            bootloader: BootloaderBuild {
                force_reinstall: false,
            },
        },
        kernels: vec![
            KernelBuild {
                id: "wasabi-kernel".into(),
                crate_path: "wasabi-kernel".to_string(),
                package_name: "wasabi-kernel".to_string(),
                features: vec![],
                no_default_feature: false,
                all_features: false,
                profile: "dev".to_string(),
                output_asm: false,
            }
            .into(),
            KernelBuild {
                id: "wasabi-test".into(),
                crate_path: "wasabi-test".to_string(),
                package_name: "wasabi-test".to_string(),
                features: vec![],
                no_default_feature: false,
                all_features: false,
                profile: "dev".to_string(),
                output_asm: false,
            }
            .into(),
        ],
        file_systems: vec![
            FileSystem {
                id: "wasabi-kernel".into(),
                fs_type: FsType::FAT,
                size: SizeBytes::new_mb(20),
                static_input: None,
                generated: vec![
                    GeneratedFsInputConfig {
                        path_in_fs: KERNEL_FILE_NAME.to_string(),
                        build: "wasabi-kernel".into(),
                        artifact: BuildArtifact::KernelElf,
                    },
                    GeneratedFsInputConfig {
                        path_in_fs: UEFI_BOOT_FILENAME.to_string(),
                        build: BuildTarget::BOOTLOADER_X86.into(),
                        artifact: BuildArtifact::KernelElf,
                    },
                ],
            }.into(),
            FileSystem {
                id: "wasabi-test".into(),
                fs_type: FsType::FAT,
                size: SizeBytes::new_mb(20),
                static_input: None,
                generated: vec![
                    GeneratedFsInputConfig {
                        path_in_fs: KERNEL_FILE_NAME.to_string(),
                        build: "wasabi-kernel".into(),
                        artifact: BuildArtifact::KernelElf,
                    },
                    GeneratedFsInputConfig {
                        path_in_fs: UEFI_BOOT_FILENAME.to_string(),
                        build: BuildTarget::BOOTLOADER_X86.into(),
                        artifact: BuildArtifact::KernelElf,
                    },
                ],
            }.into()
        ],
        images: vec![
            DiskImage {
                id:"wasabi-kernel".into(),
                partitions:vec![
                    Partition{
                        uuid: None,
                        name: "boot".to_string(),
                        fs: "wasabi-kernel".into(),
                        is_boot: true,
                        is_read_only: false
                    }
                ],
                device_uuid: None,
            }.into(),
            DiskImage {
                id:"wasabi-test".into(),
                partitions:vec![
                    Partition {
                        uuid: None,
                        name: "boot".to_string(),
                        fs: "wasabi-kernel".into(),
                        is_boot: true,
                        is_read_only: false
                    }
                ],
                device_uuid: None,
            }.into()
        ],
        tests: vec![
            TestSystem { 
                id: "histogram".into(),
                cargo: Some("histogram".into()),
                kernel: None
            }.into(),
            TestSystem { 
                id: "staticvec".into(),
                cargo: Some("staticvec".into()),
                kernel: None
            }.into(),
            TestSystem { 
                id: "shared".into(),
                cargo: Some("shared".into()),
                kernel: None
            }.into(),
            TestSystem { 
                id: "host-shared".into(),
                cargo: Some("host-shared".into()),
                kernel: None
            }.into(),
            TestSystem { 
                id: "host-wfs".into(),
                cargo: Some("host-wfs".into()),
                kernel: None
            }.into(),
            TestSystem { 
                id: "block-device".into(),
                cargo: Some("block-device".into()),
                kernel: None
            }.into(),
            TestSystem { 
                id: "wfs".into(),
                cargo: Some("wfs".into()),
                kernel: None
            }.into(),
            TestSystem { 
                id: "testing".into(),
                cargo: Some("testing".into()),
                kernel: None
            }.into(),
            TestSystem { 
                id: "testing-tests".into(),
                cargo: Some("testing".into()),
                kernel: None
            }.into(),
            TestSystem { 
                id: "gpt".into(),
                cargo: Some("gpt".into()),
                kernel: None
            }.into(),
            TestSystem { 
                id: "interrupt-fn-builder".into(),
                cargo: Some("interrupt-fn-builder".into()),
                kernel: None
            }.into(),
            TestSystem { 
                id: "logger".into(),
                cargo: Some("logger".into()),
                kernel: None
            }.into(),
            TestSystem { 
                id: "fuse".into(),
                cargo: Some("fuse".into()),
                kernel: None
            }.into(),
            TestSystem { 
                id: "wasabi-test".into(),
                cargo: None,
                kernel: Some("wasabi-test".into())
            }.into(),
            TestSystem { 
                id: "builder".into(),
                cargo: Some("builder".into()),
                kernel: None
            }.into(),
        ],
        test_groups: vec![
            TestGroup { id: "all".into(), systems: vec![], cargo_only: false, kernel_only: false }.into(),
            TestGroup { id: "owned".into(), systems: vec![
                "host-shared".into(),
                "host-wfs".into(),
                "block-device".into(),
                "gpt".into(),
                "interrupt-fn-builder".into(),
                "logger".into(),
                "shared".into(),
                "testing".into(),
                "testing-tests".into(),
                "builder".into(),
                "wasabi-test".into(),
            ], cargo_only: false, kernel_only: false }.into(),
            TestGroup { id: "cargo".into(), systems: vec![], cargo_only: true, kernel_only: false }.into(),
            TestGroup { id: "kernel".into(), systems: vec![], cargo_only: false, kernel_only: true }.into(),
        ],
        default_build_targets: vec!["wasabi-kernel".into()],
        default_tests: vec!["owned".into()],
        default_run_image: Some("wasabi-kernel".into()),
        qemu: QemuConfig {
            ovmf: Ovmf {
                prebuild_tag: "edk2-stable202605-r1".to_string(),
                prebuild_hash: "8ae4d2d73161cc2335f5675d3b8b6edfa0642301679764a246940488ea3ce20d"
                    .to_string(),
                storage_path: "ovmf".to_string(),
                download_url: "https://github.com/rust-osdev/ovmf-prebuilt/releases/download/edk2-stable202605-r1/edk2-stable202605-r1-bin.tar.xz"
                    .to_string(),
            },
            memory: "4G".to_string(),
            processor_count: 8,
            debug_log: None,
            debug_info:"int,cpu_reset,unimp,guest_errors".to_string(),
            serial: vec![ "stdio".to_string() ],
        },
        clean: CleanConfig {
            ovmf: CleanOvmf::Unused,
            build: CleanBuild {
                bootloader: false,
            },
        },
    }
}

#[cfg(test)]
mod test {
    use crate::config::initial_config;


    #[test]
    fn verify_init_config() {
        initial_config().verify().unwrap();
    }

}
