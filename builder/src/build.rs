use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use log::{debug, error, info};
use serde::{Deserialize, Serialize};
use tokio::{
    fs::{self, File},
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
};

use crate::{
    args::{BuildArgs, CheckArgs, CleanArgs},
    cargo::{cargo_version, get_workspace_manifest},
    config::{
        VerifiedConfig,
        build::{BuildArtifact, BuildConfig, BuildTarget, GeneralBuild, KernelBuild, ModuleBuild},
    },
    sync::FutureCache,
};

static BUILD_CACHE: FutureCache<BuildTarget, Result<()>> = FutureCache::const_new();

const BOOT_INFO_FILENAME: &'static str = "boot_info.ron";

pub async fn build_from_args(args: BuildArgs, config: VerifiedConfig) -> Result<()> {
    let config = Arc::new(config);
    let targets = if args.target.is_empty() {
        &config.config.default_build_targets
    } else {
        &args.target
    };
    build(targets.iter().cloned(), config.clone()).await?;
    Ok(())
}

pub async fn build_artifacts<T: IntoIterator<Item = (BuildTarget, BuildArtifact)>>(
    targets: T,
    config: Arc<VerifiedConfig>,
) -> Result<Vec<PathBuf>> {
    let mut result_paths = Vec::new();

    for (target, artifact) in targets {
        let build_config = config
            .build_targets
            .get(&target)
            .with_context(|| format!("could not find build target {target}"))?
            .clone();
        info!("build target: {target}");
        build_from_config(build_config.clone(), config.clone()).await?;

        result_paths.push(build_config.artifact_path(artifact, &config.config));
    }

    Ok(result_paths)
}

pub async fn build<T: IntoIterator<Item = BuildTarget>>(
    targets: T,
    config: Arc<VerifiedConfig>,
) -> Result<()> {
    for target in targets {
        let build_config = config
            .build_targets
            .get(&target)
            .with_context(|| format!("could not find build target {target}"))?
            .clone();
        info!("build target: {target}");
        build_from_config(build_config, config.clone()).await?;
    }
    Ok(())
}

async fn build_from_config(
    build_config: Arc<BuildConfig>,
    config: Arc<VerifiedConfig>,
) -> Result<()> {
    let build_id = build_config.id().clone();

    let build_future = async move {
        let out_dir: PathBuf = config.config.out_dir.clone().into();
        match &*build_config {
            BuildConfig::Kernel(kernel_build) => {
                build_kernel(&*kernel_build, &*config, &out_dir)
                    .await
                    .with_context(|| format!("build target kernel: {}", kernel_build.id))?;
            }
            BuildConfig::Module(module_build) => {
                build_module(&*module_build, &*config, &out_dir)
                    .await
                    .with_context(|| format!("build target module: {}", module_build.id))?;
            }
            BuildConfig::Special(id) => match id.as_str() {
                BuildTarget::BOOTLOADER_X86 => build_bootloader(&*config)
                    .await
                    .context("build bootloader-x86-64")?,
                _ => panic!("unknown special id: {id}"),
            },
        }
        Ok(())
    };

    if let Err(err) = &*BUILD_CACHE
        .spawn_or_get(build_id, build_future)
        .await
        .join()
        .await
    {
        bail!("build from config:\n{err:?}");
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
        match &**build_config {
            BuildConfig::Kernel(kernel_build) => {
                check_kernel(&*kernel_build, &config.config.general_build)
                    .await
                    .with_context(|| format!("check kernel: {target}"))?;
            }
            BuildConfig::Module(module_build) => {
                check_module(&*module_build, &config.config.general_build)
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

    let kernel_out_dir = out_dir_for(&out_dir, &kernel_config.profile, general_build);

    let elf_path = build_kernel_elf(&kernel_config, &general_build, &kernel_out_dir)
        .await
        .context("build kernel elf")?;
    if kernel_config.output_asm {
        // TODO I think I want to have an override for this flag that I
        // can trigger programatically
        let asm_out = kernel_out_dir.join(kernel_artifact_path(kernel_config, BuildArtifact::Asm));
        emit_asm(&elf_path, &asm_out)
            .await
            .context("dump kernel assembly")?;
    }

    Ok(kernel_out_dir)
}

async fn build_module(
    module_config: &ModuleBuild,
    config: &VerifiedConfig,
    out_dir: &Path,
) -> Result<PathBuf> {
    let general_build = &config.config.general_build;

    let module_out_dir = out_dir_for(&out_dir, &module_config.profile, general_build);

    let elf_path = build_module_elf(&module_config, &general_build, &module_out_dir)
        .await
        .context("build module elf")?;
    if module_config.output_asm {
        // TODO I think I want to have an override for this flag that I
        // can trigger programatically
        let asm_out = module_out_dir.join(module_artifact_path(module_config, BuildArtifact::Asm));
        emit_asm(&elf_path, &asm_out)
            .await
            .context("dump module assembly")?;
    }

    Ok(module_out_dir)
}

pub fn out_dir_for(base_dir: impl AsRef<Path>, profile: &str, build: &GeneralBuild) -> PathBuf {
    base_dir
        .as_ref()
        .join(build.architecture.kernel_tripple_str())
        .join(profile)
}

pub fn kernel_artifact_path(kernel: &KernelBuild, artifact: BuildArtifact) -> String {
    match artifact {
        BuildArtifact::KernelElf => kernel.package_name.clone(),
        BuildArtifact::Asm => format!("{}.asm", kernel.package_name),
        BuildArtifact::Lib | BuildArtifact::KernelModule | BuildArtifact::Executable => {
            panic!("Illegal artifact type for kernel build: {artifact:?}")
        }
    }
}

pub fn module_artifact_path(module: &ModuleBuild, artifact: BuildArtifact) -> String {
    match artifact {
        BuildArtifact::KernelModule => format!("lib{}.so", module.package_name),
        BuildArtifact::Asm => format!("{}.asm", module.package_name),
        BuildArtifact::KernelElf | BuildArtifact::Executable | BuildArtifact::Lib => {
            panic!("Illegal artifact type for kernel build: {artifact:?}")
        }
    }
}

fn general_cargo(cmd: &str, build_opts: &GeneralBuild, out_dir: Option<&Path>) -> Command {
    let mut command = Command::new("cargo");
    command.kill_on_drop(true);

    if let Some(jobs) = build_opts.jobs {
        command.arg("--jobs").arg(&format!("{jobs}"));
    }

    command.arg(cmd);

    // unstable-options must be passed after cmd or cargo will ignore it. I can't find
    // any documented reason for this.
    command.arg("-Z").arg("unstable-options");

    if let Some(out_dir) = out_dir {
        command.arg("--artifact-dir").arg(&out_dir);
    }

    command
}

async fn build_kernel_elf(
    kernel: &KernelBuild,
    build_opts: &GeneralBuild,
    out_dir: &Path,
) -> Result<PathBuf> {
    let mut cmd = general_cargo("build", build_opts, Some(out_dir));

    cmd.arg("-p").arg(&kernel.crate_path);

    cmd.arg("--bin").arg(&kernel.package_name);

    cmd.arg("--target")
        .arg(build_opts.architecture.kernel_tripple_str());

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

    let success = cmd
        .status()
        .await
        .context("cargo build execution")?
        .success();
    ensure!(success, "cargo build {} failed", kernel.id);

    Ok(Path::join(&out_dir, &kernel.package_name))
}

async fn check_kernel(kernel: &KernelBuild, build_opts: &GeneralBuild) -> Result<()> {
    let mut cmd = general_cargo("check", build_opts, None);

    cmd.arg("-p").arg(&kernel.crate_path);

    cmd.arg("--bin").arg(&kernel.package_name);

    cmd.arg("--target")
        .arg(build_opts.architecture.kernel_tripple_str());

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

    let success = cmd
        .status()
        .await
        .context("cargo check execution")?
        .success();
    ensure!(success, "cargo check {:?} failed", kernel.id);

    Ok(())
}

async fn build_module_elf(
    module: &ModuleBuild,
    build_opts: &GeneralBuild,
    out_dir: &Path,
) -> Result<PathBuf> {
    let mut cmd = general_cargo("build", build_opts, Some(out_dir));

    cmd.arg("-Zjson-target-spec");
    cmd.arg("-Zbuild-std=core,alloc");

    cmd.arg("-p").arg(&module.crate_path);

    cmd.arg("--lib");

    cmd.arg("--target")
        .arg(build_opts.architecture.module_tripple_str());

    if module.no_default_feature {
        cmd.arg("--no-default-features");
    }
    if module.all_features {
        cmd.arg("--all-features");
    } else {
        for feature in module.features.iter() {
            cmd.arg("--features");
            cmd.arg(&feature);
        }
    }

    cmd.arg("--profile").arg(&module.profile);

    debug!("build module using: {cmd:?}");

    let success = cmd
        .status()
        .await
        .context("cargo build execution")?
        .success();
    ensure!(success, "cargo build {} failed", module.id);

    Ok(Path::join(&out_dir, &module.package_name))
}

async fn check_module(module: &ModuleBuild, build_opts: &GeneralBuild) -> Result<()> {
    let mut cmd = general_cargo("check", build_opts, None);

    cmd.arg("-Zjson-target-spec");
    cmd.arg("-Zbuild-std=core,alloc");

    cmd.arg("-p").arg(&module.crate_path);

    cmd.arg("--lib");

    cmd.arg("--target")
        .arg(build_opts.architecture.module_tripple_str());

    if module.no_default_feature {
        cmd.arg("--no-default-features");
    }
    if module.all_features {
        cmd.arg("--all-features");
    } else {
        for feature in module.features.iter() {
            cmd.arg("--features");
            cmd.arg(&feature);
        }
    }

    cmd.arg("--profile").arg(&module.profile);

    let success = cmd
        .status()
        .await
        .context("cargo check execution")?
        .success();
    ensure!(success, "cargo check {:?} failed", module.id);

    Ok(())
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
struct BootloaderInfo {
    version: String,
    git: Option<String>,
    branch: Option<String>,
    tag: Option<String>,
    rev: Option<String>,
    no_default_features: bool,
    features: Vec<String>,
    cargo_version: String,
}

pub async fn build_bootloader(config: &VerifiedConfig) -> Result<()> {
    let out_dir = bootloader_out_dir(&config.config.out_dir, &BuildTarget::BOOTLOADER_X86.into());

    let (cargo_version, manifest) = tokio::try_join!(cargo_version(), get_workspace_manifest())?;

    let bootloader = manifest
        .workspace
        .as_ref()
        .context("get workspace manifest")?
        .dependencies
        .get("bootloader-x86_64-uefi")
        .context("get bootloader dependency")?;

    let info = match bootloader {
        cargo_toml::Dependency::Simple(version) => BootloaderInfo {
            version: version.clone(),
            git: None,
            branch: None,
            tag: None,
            rev: None,
            no_default_features: false,
            features: Vec::new(),
            cargo_version,
        },
        cargo_toml::Dependency::Inherited(_inherited_dependency_detail) => bail!(
            "bootloader dependency is inherited. That should not be possible for a workspace dep"
        ),
        cargo_toml::Dependency::Detailed(dep) => {
            ensure!(
                dep.registry.is_none(),
                "bootloader dependency is not supported"
            );
            ensure!(
                dep.registry_index.is_none(),
                "bootloader dependency is not supported"
            );
            ensure!(dep.path.is_none(), "bootloader dependency is not supported");

            BootloaderInfo {
                version: dep
                    .version
                    .clone()
                    .ok_or(anyhow!("bootloader dependency has no version defined"))?,
                git: dep.git.clone(),
                branch: dep.branch.clone(),
                tag: dep.tag.clone(),
                rev: dep.rev.clone(),
                no_default_features: !dep.default_features,
                features: dep.features.clone(),
                cargo_version,
            }
        }
    };
    if !should_reinstall_bootloadert(config, &info).await {
        info!("reusing existing bootloader");
        return Ok(());
    }

    let mut cmd = general_cargo("install", &config.config.general_build, None);
    cmd.arg("bootloader-x86_64-uefi");

    cmd.arg("-Z").arg("bindeps");

    cmd.arg("--version").arg(&info.version);
    if let Some(git) = info.git.as_ref() {
        cmd.arg("--git").arg(git);
        if let Some(branch) = info.branch.as_ref() {
            cmd.arg("--branch").arg(branch);
        }
        if let Some(tag) = info.tag.as_ref() {
            cmd.arg("--tag").arg(tag);
        }
        if let Some(rev) = info.rev.as_ref() {
            cmd.arg("--rev").arg(rev);
        }
    }
    if info.no_default_features {
        cmd.arg("--no-default-features");
    }
    for feature in &info.features {
        cmd.arg("--features").arg(feature);
    }
    cmd.arg("--locked");
    cmd.arg("--target").arg("x86_64-unknown-uefi");
    cmd.arg("-Zbuild-std=core")
        .arg("-Zbuild-std-features=compiler-builtins-mem");
    cmd.arg("--root").arg(&out_dir);
    cmd.env_remove("RUSTFLAGS");
    cmd.env_remove("CARGO_ENCODED_RUSTFLAGS");

    debug!("{cmd:?}");

    let success = cmd.status().await.context("wait for install")?.success();
    ensure!(success, "failed to install uefi bootloader");

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

async fn should_reinstall_bootloadert(
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
