extern crate wasabi_os;
use std::{
    ffi::{OsStr, OsString},
    fs::{remove_dir_all, File},
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    str::FromStr,
    time::Duration,
};

use cargo_toml::Manifest;
use clap::{Args, Parser, Subcommand, ValueEnum};
use wasabi_os::{launch_qemu, launch_with_timeout, Kernel, QemuConfig};

/// Runner for WasabiOs
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Arguments {
    #[command(subcommand)]
    build: BuildCommand,
}

#[derive(Debug, Subcommand)]
enum BuildCommand {
    /// rebuild the kernel
    Build(BuildArgs),
    /// use the latest available build. Fails if no build exists
    Latest(LatestArgs),
    /// Cleanup build artifacts
    Clean(CleanArgs),
    /// run cargo check on kernel
    Check(CheckArgs),
}

#[derive(Debug, Subcommand)]
enum RunCommand {
    /// run the kernel in qemu
    Run,
    /// run kernel tests in qemu
    Test(TestArgs),
}

#[derive(Args, Debug)]
struct TestArgs {
    /// fails qemu tests after `timeout` seconds.
    #[arg(long, default_value_t = 10)]
    timeout: u64,
}

#[derive(Args, Debug)]
struct BuildArgs {
    #[command(flatten)]
    options: BuildOptions,

    /// skip building tests, this cannot be used with the "test" command
    #[arg(long)]
    no_tests: bool,

    #[arg(long)]
    emit_asm: bool,

    #[command(subcommand)]
    run: Option<RunCommand>,
}

#[derive(Args, Debug)]
struct LatestArgs {
    #[arg(short, long)]
    profile: Profile,

    /// target architecture
    #[arg(long, default_value = Target::X86_64)]
    target: Target,

    #[command(subcommand)]
    run: RunCommand,
}

#[derive(Args, Debug)]
struct CleanArgs {
    #[arg(long, short)]
    workspace: bool,

    #[arg(long, short)]
    kernel: bool,

    #[arg(long, short)]
    test: bool,

    #[arg(long, short)]
    latest: bool,

    #[arg(long, short)]
    all: bool,

    #[arg(long)]
    docs: bool,
}

#[derive(Args, Debug)]
struct CheckArgs {
    #[command(flatten)]
    options: BuildOptions,

    /// skip building tests, this cannot be used with the "test" command
    #[arg(long)]
    no_tests: bool,
}

#[derive(Args, Debug)]
struct BuildOptions {
    /// build profile
    #[arg(short, long)]
    profile: Option<Profile>,

    /// use release profile, overwritten by [profile]
    #[arg(short, long)]
    release: bool,

    /// target architecture
    #[arg(long, default_value = Target::X86_64)]
    target: Target,

    /// build features
    #[arg(long)]
    features: Vec<Feature>,

    /// build with all features, overwrites [features]
    #[arg(long)]
    all_features: bool,

    /// disable default features. Features can be reenabled with [features]
    #[arg(long)]
    no_default_features: bool,
}

impl BuildOptions {
    fn profile(&self) -> Profile {
        self.profile.unwrap_or_else(|| {
            if self.release {
                Profile::Release
            } else {
                Profile::Dev
            }
        })
    }
}

/// Build the OS with the specified Profile (default debug)
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
enum Profile {
    /// Dev profile (default)
    Dev,
    /// Release profile
    Release,
}

impl Profile {
    fn as_os_str(&self) -> &'static OsStr {
        match self {
            Profile::Dev => OsStr::new("dev"),
            Profile::Release => OsStr::new("release"),
        }
    }
}

/// Target architecture (default X86_64)
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
enum Target {
    /// X86-64
    X86_64,
}

impl Into<clap::builder::OsStr> for Target {
    fn into(self) -> clap::builder::OsStr {
        match self {
            Target::X86_64 => "x86-64".into(),
        }
    }
}

impl Target {
    fn tripple_str(&self) -> &'static OsStr {
        match self {
            Target::X86_64 => OsStr::new("x86_64-unknown-none"),
        }
    }
}

/// Build feature used for `cfg`
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
enum Feature {
    NoUnicodeLog,
    NoColor,
}

impl Feature {
    fn as_os_str(&self) -> &'static OsStr {
        match self {
            Feature::NoUnicodeLog => OsStr::new("no-unicode-log"),
            Feature::NoColor => OsStr::new("no-color"),
        }
    }
}

fn main() {
    let args = Arguments::parse();

    match args.build {
        BuildCommand::Build(args) => build(args),
        BuildCommand::Latest(args) => latests(args),
        BuildCommand::Clean(args) => clean(args),
        BuildCommand::Check(args) => check(args),
    }
}

fn check(args: CheckArgs) {
    check_kernel(OsStr::new("wasabi-kernel"), &args.options);
    if !args.no_tests {
        check_kernel(OsStr::new("wasabi-test"), &args.options);
    }
}

fn build(args: BuildArgs) {
    // TODO build everything in temp dir and only on success copy to latest
    let mut to_run: PathBuf;

    let (elf, dir) = build_kernel_elf(OsStr::new("wasabi-kernel"), &args.options);
    let img = build_kernel_uefi(&elf, Path::join(&dir, "wasabi-kernel-uefi.img"));
    if args.emit_asm {
        emit_asm(&elf, &Path::join(&dir, "wasabi-kernel.asm"));
    }
    to_run = img.to_owned();

    if !args.no_tests {
        let (elf, dir) = build_kernel_elf(OsStr::new("wasabi-test"), &args.options);
        let img = build_kernel_uefi(&elf, Path::join(&dir, "wasabi-test-uefi.img"));
        if args.emit_asm {
            emit_asm(&elf, &Path::join(&dir, "wasabi-test.asm"));
        }

        if let Some(RunCommand::Test(_)) = args.run {
            to_run = img.to_owned();
        }
    }

    match args.run {
        Some(RunCommand::Run) => run(&to_run),
        Some(RunCommand::Test(args)) => test(&to_run, args),
        _ => {}
    }
}

fn build_kernel_uefi(elf: &Path, img_path: PathBuf) -> PathBuf {
    bootloader::UefiBoot::new(elf)
        .create_disk_image(&img_path)
        .unwrap();
    img_path
}

fn emit_asm(img: &Path, asm_file_path: &Path) {
    let mut cmd = Command::new("objdump");
    cmd.arg("-M").arg("intel");
    cmd.arg("-d").arg(img);

    cmd.stdout(Stdio::piped());

    let output = cmd.spawn().unwrap().wait_with_output().unwrap();
    assert!(output.status.success(), "Objdump failed for {:?}", img);

    let mut asm_file = File::create(asm_file_path).unwrap();
    asm_file
        .write(&output.stdout)
        .expect(&format!("Writing asm file failed {:?}", &asm_file_path));
}

fn build_kernel_elf(bin_name: &OsStr, options: &BuildOptions) -> (PathBuf, PathBuf) {
    let mut cmd = Command::new("cargo");
    cmd.arg("build");

    cmd.arg("-p").arg(bin_name);

    cmd.arg("--bin").arg(bin_name);

    cmd.arg("-Z").arg("unstable-options");

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

    let out_dir = latest_path(bin_name, &options.target, &options.profile());
    cmd.arg("--out-dir").arg(&out_dir);

    assert!(
        cmd.spawn().unwrap().wait().unwrap().success(),
        "cargo build {:?} failed",
        bin_name
    );

    (Path::join(&out_dir, bin_name), out_dir)
}

fn check_kernel(bin_name: &OsStr, options: &BuildOptions) {
    let mut cmd = Command::new("cargo");
    cmd.arg("check");

    cmd.arg("-p").arg(bin_name);

    cmd.arg("--bin").arg(bin_name);

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

    assert!(
        cmd.spawn().unwrap().wait().unwrap().success(),
        "cargo check {:?} failed",
        bin_name
    );
}

fn latest_path(bin_name: &OsStr, target: &Target, profile: &Profile) -> PathBuf {
    let mut path = PathBuf::new();
    path.push("latest");
    path.push(target.tripple_str());
    path.push(profile.as_os_str());
    path.push(bin_name);

    path
}

fn run(uefi: &Path) {
    let kernel = Kernel {
        path: &uefi,
        uefi: true,
    };

    let qemu = QemuConfig::default();

    launch_qemu(kernel, qemu).wait().unwrap();
}

fn test(uefi: &Path, args: TestArgs) {
    const SUCCESS: i32 = 0x101;
    const FAILURE: i32 = 0x111;

    let kernel = Kernel {
        path: &uefi,
        uefi: true,
    };

    let qemu = QemuConfig {
        devices: "isa-debug-exit,iobase=0xf4,iosize=0x04",
        ..QemuConfig::default()
    };
    match launch_with_timeout(Duration::from_secs(args.timeout), kernel, qemu) {
        Ok(exit_status) => {
            if exit_status.success() {
                panic!("Qemu exit with code 0, but we expected {}", SUCCESS);
            } else {
                match exit_status.code() {
                    Some(SUCCESS) => println!("Tests finished successfully"),
                    Some(FAILURE) => panic!("Tests failed"),
                    Some(code) => panic!("Qemu exited with unexpected exit code {}", code),
                    None => panic!("Qemu did not succeed, but has no error code????"),
                }
            }
        }
        Err(err) => panic!("Launching Qemu failed: {err:?}"),
    }
}

fn latests(args: LatestArgs) {
    let mut bin_name = OsString::from_str(match args.run {
        RunCommand::Run => "wasabi-kernel",
        RunCommand::Test(_) => "wasabi-test",
    })
    .unwrap();

    let mut path = latest_path(&bin_name, &args.target, &args.profile);

    bin_name.push("_uefi.img");
    path.push(&bin_name);
    match args.run {
        RunCommand::Run => run(&path),
        RunCommand::Test(args) => test(&path, args),
    }
}

fn clean(args: CleanArgs) {
    if args.docs {
        let mut cmd = Command::new("cargo");
        cmd.arg("clean").arg("--docs");
        assert!(
            cmd.spawn().unwrap().wait().unwrap().success(),
            "cargo clean --docs failed"
        );
        return;
    }

    if args.all {
        let mut cmd = Command::new("cargo");
        cmd.arg("clean");
        assert!(
            cmd.spawn().unwrap().wait().unwrap().success(),
            "cargo clean failed"
        );
        return;
    }

    if args.kernel {
        let mut cmd = Command::new("cargo");
        cmd.arg("clean");
        cmd.arg("-p").arg("wasabi-kernel");
        assert!(
            cmd.spawn().unwrap().wait().unwrap().success(),
            "clean kernel failed"
        );
    }

    if args.test {
        let mut cmd = Command::new("cargo");
        cmd.arg("clean");
        cmd.arg("-p").arg("wasabi-kernel");
        assert!(
            cmd.spawn().unwrap().wait().unwrap().success(),
            "clean tests failed"
        );
    }

    if args.latest {
        match remove_dir_all("latest") {
            Ok(_) => {}
            Err(e) => println!("failed to clean latest because {:?}", e),
        }
    }

    if args.workspace {
        let manifest = Manifest::from_path("Cargo.toml").unwrap();
        for member in &manifest.workspace.unwrap().members {
            let mut cmd = Command::new("cargo");
            cmd.arg("clean");
            cmd.arg("-p").arg(member.replace("/", "-"));
            assert!(
                cmd.spawn().unwrap().wait().unwrap().success(),
                "clean member {} failed",
                member
            );
        }
    }
}
