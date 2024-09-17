use crate::{
    args::{BuildArgs, BuildOptions, CheckArgs, CleanArgs, ExpandArgs, RunCommand},
    latest_path, run,
    test::test,
};
use anyhow::{bail, ensure, Context, Result};
use cargo_toml::Manifest;
use std::{
    ffi::OsStr,
    fs::{remove_dir_all, File},
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
};
use tokio::process::Command;

pub enum KernelBinary {
    Wasabi,
    Test,
}

impl KernelBinary {
    pub fn name(&self) -> &'static OsStr {
        match self {
            KernelBinary::Wasabi => OsStr::new("wasabi-kernel"),
            KernelBinary::Test => OsStr::new("wasabi-test"),
        }
    }
}

pub async fn check(args: CheckArgs) -> Result<()> {
    check_kernel(KernelBinary::Wasabi, &args.options)
        .await
        .context("check kernel")?;
    if !args.no_tests {
        check_kernel(KernelBinary::Test, &args.options)
            .await
            .context("check test")?;
    }
    Ok(())
}

pub async fn build(args: BuildArgs) -> Result<()> {
    // TODO build everything in temp dir and only on success copy to latest
    let mut to_run: PathBuf;

    let (elf, dir) = build_kernel_elf(KernelBinary::Wasabi, &args.options)
        .await
        .context("build kernel elf")?;
    let img = build_kernel_uefi(&elf, Path::join(&dir, "wasabi-kernel-uefi.img"))
        .context("build kernel")?;
    if args.emit_asm {
        emit_asm(&elf, &Path::join(&dir, "wasabi-kernel.asm"))
            .await
            .context("emit kernel asm")?;
    }
    to_run = img.to_owned();

    if !args.no_tests {
        let (elf, dir) = build_kernel_elf(KernelBinary::Test, &args.options)
            .await
            .context("build test kernel elf")?;
        let img = build_kernel_uefi(&elf, Path::join(&dir, "wasabi-test-uefi.img"))
            .context("build test kernel")?;
        if args.emit_asm {
            emit_asm(&elf, &Path::join(&dir, "wasabi-test.asm"))
                .await
                .context("emit test asm")?;
        }

        if let Some(RunCommand::Test(_)) = args.run {
            to_run = img.to_owned();
        }
    }

    match args.run {
        Some(RunCommand::Run(args)) => run(&to_run, args).await,
        Some(RunCommand::Test(args)) => test(&to_run, args).await,
        _ => Ok(()),
    }
}

pub fn build_kernel_uefi(elf: &Path, img_path: PathBuf) -> Result<PathBuf> {
    bootloader::UefiBoot::new(elf)
        .create_disk_image(&img_path)
        .context("create uefi disk image")?;
    Ok(img_path)
}

pub async fn emit_asm(img: &Path, asm_file_path: &Path) -> Result<()> {
    let mut cmd = Command::new("objdump");
    cmd.arg("-M").arg("intel");
    cmd.arg("-d").arg(img);

    cmd.stdout(Stdio::piped());

    let output = cmd
        .spawn()
        .context("spawn objdump")?
        .wait_with_output()
        .await
        .context("objdump")?;
    ensure!(output.status.success(), "Objdump failed for {:?}", img);

    let mut asm_file = File::create(asm_file_path).context("asm file creation")?;
    asm_file
        .write(&output.stdout)
        .context(format!("Writing asm file failed {:?}", &asm_file_path))?;

    Ok(())
}

pub async fn build_kernel_elf(
    bin: KernelBinary,
    options: &BuildOptions,
) -> Result<(PathBuf, PathBuf)> {
    let mut cmd = Command::new("cargo");
    cmd.arg("build");

    let bin_name = bin.name();
    cmd.arg("-p").arg(bin_name);

    cmd.arg("--bin").arg(bin_name);

    cmd.arg("-Z").arg("unstable-options");

    cmd.arg("--target").arg(options.target.tripple_str());

    if options.no_default_features {
        cmd.arg("--no-default-features");
    }
    if options.all_features {
        cmd.arg("--all-features");
    } else if options.features.iter().any(|f| f.used_in(&bin)) {
        cmd.arg("--features");
        for feature in options.features.iter().filter(|f| f.used_in(&bin)) {
            cmd.arg(feature.as_os_str());
        }
    }

    cmd.arg("--profile").arg(options.profile().as_os_str());

    let out_dir = latest_path(bin_name, &options.target, &options.profile());
    cmd.arg("--artifact-dir").arg(&out_dir);

    let success = cmd
        .status()
        .await
        .context("cargo build execution")?
        .success();
    ensure!(success, "cargo build {:?} failed", bin_name);

    Ok((Path::join(&out_dir, bin_name), out_dir))
}

async fn check_kernel(bin: KernelBinary, options: &BuildOptions) -> Result<()> {
    let mut cmd = Command::new("cargo");
    cmd.arg("check");

    let bin_name = bin.name();
    cmd.arg("-p").arg(bin_name);

    cmd.arg("--bin").arg(bin_name);

    cmd.arg("--target").arg(options.target.tripple_str());

    if options.no_default_features {
        cmd.arg("--no-default-features");
    }
    if options.all_features {
        cmd.arg("--all-features");
    } else if options.features.iter().any(|f| f.used_in(&bin)) {
        cmd.arg("--features");
        for feature in options.features.iter().filter(|f| f.used_in(&bin)) {
            cmd.arg(feature.as_os_str());
        }
    }

    cmd.arg("--profile").arg(options.profile().as_os_str());

    let success = cmd
        .status()
        .await
        .context("cargo check execution")?
        .success();
    ensure!(success, "cargo check {:?} failed", bin_name);
    Ok(())
}

pub async fn clean(args: CleanArgs) -> Result<()> {
    if args.docs {
        let mut cmd = Command::new("cargo");
        cmd.arg("clean").arg("--docs");
        return cmd
            .status()
            .await
            .context("cargo clean --docs execution")?
            .exit_ok()
            .context("cargo clean --docs failed");
    }

    if args.all {
        let mut cmd = Command::new("cargo");
        cmd.arg("clean");
        return cmd
            .status()
            .await
            .context("cargo clean execution")?
            .exit_ok()
            .context("cargo clean failec");
    }

    if args.kernel {
        let mut cmd = Command::new("cargo");
        cmd.arg("clean");
        cmd.arg("-p").arg("wasabi-kernel");
        return cmd
            .status()
            .await
            .context("cargo clean -p kernel execution")?
            .exit_ok()
            .context("cargo clean -p kernel failec");
    }

    if args.test {
        let mut cmd = Command::new("cargo");
        cmd.arg("clean");
        cmd.arg("-p").arg("wasabi-test");
        return cmd
            .status()
            .await
            .context("cargo clean -p test execution")?
            .exit_ok()
            .context("cargo clean -p test failec");
    }

    if args.latest {
        match remove_dir_all("latest") {
            Ok(_) => {}
            Err(e) => bail!("failed to clean latest because {:?}", e),
        }
    }

    if args.ovmf {
        match remove_dir_all("target/ovmf") {
            Ok(_) => {}
            Err(e) => bail!("failed to clean ovmf prebuild binaries because {:?}", e),
        }
    }

    if args.workspace {
        let manifest = Manifest::from_path("Cargo.toml").context("failed to parse Cargo.toml")?;
        for member in &manifest
            .workspace
            .context("no workspaces in Cargo.toml")?
            .members
        {
            let mut cmd = Command::new("cargo");
            cmd.arg("clean");
            cmd.arg("-p").arg(member.replace("/", "-"));
            let success = cmd
                .status()
                .await
                .context(format!("cargo clean -p {} execution", member))?
                .success();
            ensure!(success, "clean member {} failed", member);
        }
    }
    Ok(())
}

pub async fn expand(args: ExpandArgs) -> Result<()> {
    let mut cmd = Command::new("cargo");
    cmd.arg("expand");

    cmd.arg("--package").arg(args.package.as_os_path());

    if let Some(bin) = args.bin {
        if args.lib {
            bail!("Only one of --lib and --bin can be used at once");
        }
        cmd.arg("--bin").arg(bin);
    } else if args.lib {
        cmd.arg("--lib");
    }

    let options = &args.options;
    cmd.arg("--target").arg(options.target.tripple_str());

    if options.no_default_features {
        cmd.arg("--no-default-features");
    }
    if options.all_features {
        cmd.arg("--all-features");
    } else if !options.features.is_empty() {
        cmd.arg("--features");
        for feature in &options.features {
            cmd.arg(feature.as_os_str());
        }
    }

    cmd.arg("--profile").arg(options.profile().as_os_str());

    if let Some(item) = args.item {
        cmd.arg(item);
    }

    let success = cmd
        .status()
        .await
        .context("cargo expand execution")?
        .success();
    ensure!(success, "cargo expand {:?} failed", args.package);
    Ok(())
}
