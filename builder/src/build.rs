use std::{
    path::{Path, PathBuf},
    process::Stdio,
};

use anyhow::{Context, Result, ensure};
use log::{debug, error, info};
use serde::{Deserialize, Serialize};
use tokio::{
    fs::{self, File},
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
};

use crate::{
    args::{BuildArgs, CheckArgs, CleanArgs},
    cargo::get_workspace_manifest,
    config::{
        VerifiedConfig,
        build::{BuildArtifact, BuildConfig, BuildTarget, GeneralBuild, KernelBuild},
    },
};

const BOOT_INFO_FILENAME: &'static str = "boot_info.ron";

pub async fn build_from_args(args: BuildArgs, config: VerifiedConfig) -> Result<()> {
    let targets = if args.target.is_empty() {
        &config.config.default_build_targets
    } else {
        &args.target
    };
    build(targets.iter().cloned(), &config).await?;
    Ok(())
}

pub async fn build_artifacts<T: IntoIterator<Item = (BuildTarget, BuildArtifact)>>(
    targets: T,
    config: &VerifiedConfig,
) -> Result<Vec<PathBuf>> {
    let mut result_paths = Vec::new();

    for (target, artifact) in targets {
        let build_config = config
            .build_targets
            .get(&target)
            .with_context(|| format!("could not find build target {target}"))?;
        info!("build target: {target}");
        build_from_config(build_config, config).await?;

        result_paths.push(build_config.artifact_path(artifact, &config.config));
    }

    Ok(result_paths)
}

pub async fn build<T: IntoIterator<Item = BuildTarget>>(
    targets: T,
    config: &VerifiedConfig,
) -> Result<()> {
    for target in targets {
        let build_config = config
            .build_targets
            .get(&target)
            .with_context(|| format!("could not find build target {target}"))?;
        info!("build target: {target}");
        build_from_config(build_config, config).await?;
    }
    Ok(())
}

async fn build_from_config(build_config: &BuildConfig, config: &VerifiedConfig) -> Result<()> {
    let out_dir: PathBuf = config.config.out_dir.clone().into();
    match build_config {
        BuildConfig::Kernel(kernel_build) => {
            build_kernel(&*kernel_build, &config, &out_dir)
                .await
                .with_context(|| format!("build target: {}", kernel_build.id))?;
        }
        BuildConfig::Special(id) => match id.as_str() {
            BuildTarget::BOOTLOADER_X86 => build_bootloader(config)
                .await
                .context("build bootloader-x86-64")?,
            _ => panic!("unknown special id: {id}"),
        },
    }
    Ok(())
}

pub async fn check(args: CheckArgs, config: VerifiedConfig) -> Result<()> {
    let targets = if args.target.is_empty() {
        &config.config.default_build_targets
    } else {
        &args.target
    };

    for target in targets {
        let build_config = config
            .build_targets
            .get(target)
            .with_context(|| format!("could not find build target {target}"))?;
        info!("build target: {target}");
        match build_config {
            BuildConfig::Kernel(kernel_build) => {
                check_kernel(&*kernel_build, &config.config.general_build)
                    .await
                    .with_context(|| format!("check kernel: {target}"))?;
            }
            BuildConfig::Special(id) => match id.as_str() {
                BuildTarget::BOOTLOADER_X86 => {
                    // external codebase on crates.io. No need to check anything
                }
                _ => panic!("unknown special id: {id}"),
            },
        }
    }
    Ok(())
}

async fn build_kernel(
    kernel_config: &KernelBuild,
    config: &VerifiedConfig,
    out_dir: &Path,
) -> Result<PathBuf> {
    let general_build = &config.config.general_build;

    let kernel_out_dir = out_dir_for(&out_dir, &kernel_config, general_build);

    let elf_path = build_kernel_elf(&kernel_config, &general_build, &kernel_out_dir)
        .await
        .context("build kernel elf")?;
    if kernel_config.output_asm {
        // TODO I think I want to have an override for this flag that I
        // can trigger programatically
        let asm_out = kernel_out_dir.join(artifact_path(kernel_config, BuildArtifact::Asm));
        emit_asm(&elf_path, &asm_out)
            .await
            .context("dump kernel assembly")?;
    }

    Ok(kernel_out_dir)
}

pub fn out_dir_for(
    base_dir: impl AsRef<Path>,
    kernel: &KernelBuild,
    build: &GeneralBuild,
) -> PathBuf {
    base_dir
        .as_ref()
        .join(build.architecture.tripple_str())
        .join(&kernel.profile)
}

pub fn artifact_path(kernel: &KernelBuild, artifact: BuildArtifact) -> String {
    match artifact {
        BuildArtifact::KernelElf => kernel.package_name.clone(),
        BuildArtifact::Asm => format!("{}.asm", kernel.package_name),
        BuildArtifact::Lib => format!("{}.rlib", kernel.package_name),
        BuildArtifact::KernelModule => todo!(),
        BuildArtifact::Executable => todo!(),
    }
}

async fn build_kernel_elf(
    kernel: &KernelBuild,
    build_opts: &GeneralBuild,
    out_dir: &Path,
) -> Result<PathBuf> {
    let mut cmd = Command::new("cargo");
    cmd.kill_on_drop(true);
    cmd.arg("build");

    cmd.arg("-p").arg(&kernel.crate_path);

    cmd.arg("--bin").arg(&kernel.package_name);

    cmd.arg("-Z").arg("unstable-options");

    cmd.arg("--target")
        .arg(build_opts.architecture.tripple_str());

    if kernel.no_default_feature {
        cmd.arg("--no-default-features");
    }
    if kernel.all_features {
        cmd.arg("--all-features");
    } else {
        for feature in kernel.features.iter() {
            cmd.arg("--features");
            cmd.arg(&feature);
        }
    }

    cmd.arg("--profile").arg(&kernel.profile);

    if let Some(jobs) = build_opts.jobs {
        cmd.arg("--jobs").arg(&format!("{jobs}"));
    }

    cmd.arg("--artifact-dir").arg(&out_dir);

    let success = cmd
        .status()
        .await
        .context("cargo build execution")?
        .success();
    ensure!(success, "cargo build {:?} failed", kernel.id);

    Ok(Path::join(&out_dir, &kernel.package_name))
}

async fn check_kernel(kernel: &KernelBuild, build_opts: &GeneralBuild) -> Result<()> {
    let mut cmd = Command::new("cargo");
    cmd.kill_on_drop(true);
    cmd.arg("check");

    cmd.arg("-p").arg(&kernel.crate_path);

    cmd.arg("--bin").arg(&kernel.package_name);

    cmd.arg("-Z").arg("unstable-options");

    cmd.arg("--target")
        .arg(build_opts.architecture.tripple_str());

    if kernel.no_default_feature {
        cmd.arg("--no-default-features");
    }
    if kernel.all_features {
        cmd.arg("--all-features");
    } else {
        for feature in kernel.features.iter() {
            cmd.arg("--features");
            cmd.arg(&feature);
        }
    }

    cmd.arg("--profile").arg(&kernel.profile);

    if let Some(jobs) = build_opts.jobs {
        cmd.arg("--jobs").arg(&format!("{jobs}"));
    }

    let success = cmd
        .status()
        .await
        .context("cargo check execution")?
        .success();
    ensure!(success, "cargo check {:?} failed", kernel.id);

    Ok(())
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
struct BootloaderInfo {
    version: String,
}

pub async fn build_bootloader(config: &VerifiedConfig) -> Result<()> {
    let out_dir = bootloader_out_dir(&config.config.out_dir, &BuildTarget::BOOTLOADER_X86.into());

    let manifest = get_workspace_manifest().await?;
    let bootloader = manifest
        .workspace
        .as_ref()
        .unwrap()
        .dependencies
        .get("bootloader")
        .unwrap();

    let version = bootloader.req();
    let info = BootloaderInfo {
        version: version.to_string(),
    };
    if !should_download_bootloader(config, &info).await {
        info!("reusing existing bootloader");
        return Ok(());
    }

    let mut cmd = Command::new("cargo");
    cmd.kill_on_drop(true);

    if let Some(jobs) = config.config.general_build.jobs {
        cmd.arg("--jobs").arg(&format!("{jobs}"));
    }

    cmd.arg("install").arg("bootloader-x86_64-uefi");
    cmd.arg("--version").arg(version);
    cmd.arg("--locked");
    cmd.arg("--target").arg("x86_64-unknown-uefi");
    cmd.arg("-Zbuild-std=core")
        .arg("-Zbuild-std-features=compiler-builtins-mem");
    cmd.arg("--root").arg(&out_dir);
    cmd.env_remove("RUSTFLAGS");
    cmd.env_remove("CARGO_ENCODED_RUSTFLAGS");
    let success = cmd
        .status()
        .await
        .context("install uefi x86-64 bootloader")?
        .success();
    ensure!(success, "failed to build uefi bootloader");

    let info_file = out_dir.join(BOOT_INFO_FILENAME);
    let mut info_file = File::create(info_file)
        .await
        .context("create bootloader info file")?;
    let info = ron::ser::to_string_pretty(&info, Default::default())
        .context("serialize bootloader info")?;
    info_file
        .write_all(info.as_bytes())
        .await
        .context("write bootloader info to file")?;
    info_file
        .flush()
        .await
        .context("flush bootloader info file")?;

    Ok(())
}

async fn should_download_bootloader(
    config: &VerifiedConfig,
    requirements: &BootloaderInfo,
) -> bool {
    let id = BuildTarget::BOOTLOADER_X86.into();
    let out_dir = bootloader_out_dir(&config.config.out_dir, &id);
    let info_path = out_dir.join(BOOT_INFO_FILENAME);
    let bin = out_dir.join(bootloader_artifact_path(BuildArtifact::KernelElf, &id));

    if !bin.exists() || !info_path.exists() {
        return true;
    }

    let mut info_file = match File::open(&info_path).await.context("open info file") {
        Ok(info_file) => info_file,
        Err(err) => {
            error!("could not open info file at {}\n{err}", info_path.display());
            return true;
        }
    };
    let mut info_data = String::new();
    if let Err(err) = info_file
        .read_to_string(&mut info_data)
        .await
        .context("read info file")
    {
        error!(
            "failed to read from info file at {}\n{err}",
            info_path.display()
        );
        return true;
    }
    let stored_info: BootloaderInfo = match ron::from_str(&info_data) {
        Ok(data) => data,
        Err(err) => {
            error!("invalid boot info data at: {}\n{err}", info_path.display());
            return true;
        }
    };

    &stored_info != requirements
}

pub fn bootloader_out_dir<P: AsRef<Path>>(base_path: P, target: &BuildTarget) -> PathBuf {
    assert_eq!(target.as_str(), BuildTarget::BOOTLOADER_X86);
    base_path.as_ref().join("bootloader").join("x86")
}

pub fn bootloader_artifact_path(artifact: BuildArtifact, target: &BuildTarget) -> String {
    assert_eq!(target.as_str(), BuildTarget::BOOTLOADER_X86);
    assert_eq!(artifact, BuildArtifact::KernelElf);
    "bin/bootloader-x86_64-uefi.efi".into()
}

async fn emit_asm(elf: &Path, asm_out: &Path) -> Result<()> {
    let mut cmd = Command::new("objdump");
    cmd.kill_on_drop(true);
    cmd.arg("-M").arg("intel");
    cmd.arg("-d").arg(elf);
    cmd.arg("--demangle=rust");

    cmd.stdout(Stdio::piped());

    let output = cmd
        .spawn()
        .context("spawn objdump")?
        .wait_with_output()
        .await
        .context("objdump")?;
    ensure!(
        output.status.success(),
        "Objdump failed for {}",
        elf.display()
    );

    if let Some(parent) = asm_out.parent() {
        fs::create_dir_all(parent)
            .await
            .context("create parent dir for fat disk image")?;
    }
    let mut asm_file = File::create(asm_out).await.context("asm file creation")?;
    asm_file
        .write(&output.stdout)
        .await
        .context(format!("Writing asm file failed {:?}", &asm_out))?;

    info!("dump asm to {}", asm_out.display());

    Ok(())
}

pub async fn clean(clean: &CleanArgs, config: &VerifiedConfig) -> Result<()> {
    debug!("clean");
    for build_target in config.build_targets.values() {
        if skip_clean(build_target, clean, config) {
            continue;
        }
        let out_dir = build_target.out_dir(&config.config.out_dir, &config.config.general_build);
        if out_dir.exists() {
            fs::remove_dir_all(out_dir)
                .await
                .with_context(|| format!("delete out dir for {}", build_target.id()))?;
        }
    }

    Ok(())
}

fn skip_clean(build: &BuildConfig, args: &CleanArgs, config: &VerifiedConfig) -> bool {
    if args.all {
        return false;
    }
    let BuildConfig::Special(special_target) = build else {
        return false;
    };

    let clean_conf = &config.config.clean.build;

    match special_target.as_str() {
        BuildTarget::BOOTLOADER_X86 => !clean_conf.bootloader,
        _ => false,
    }
}
