mod ovmf;

use anyhow::{bail, Context, Result};
use ovmf::OvmfPaths;
use std::{
    ffi::{OsStr, OsString},
    net::Ipv4Addr,
    os::unix::prelude::OsStrExt,
    path::Path,
    process::ExitStatus,
    time::Duration,
};
use tokio::{
    fs::File,
    io::{AsyncBufReadExt, BufReader},
    process::{Child, Command},
    time,
};

use crate::args::{QemuOptions, UefiOptions};

pub enum Arch {
    X86_64,
}

impl Arch {
    fn qemu_extension(&self) -> &'static str {
        match self {
            Arch::X86_64 => "-system-x86_64",
        }
    }
}

#[derive(Debug)]
pub struct Kernel<'a> {
    pub path: &'a Path,
}

#[derive(Debug, Default)]
pub struct UefiConfig<'a> {
    pub ovmf_code: Option<&'a Path>,
    pub ovmf_vars: Option<&'a Path>,
}

impl<'a> From<&'a UefiOptions> for UefiConfig<'a> {
    fn from(value: &'a UefiOptions) -> Self {
        let mut uefi = UefiConfig::default();
        if let Some(code) = value.ovmf_code.as_ref() {
            uefi.ovmf_code = Some(&code);
        }
        if let Some(vars) = value.ovmf_vars.as_ref() {
            uefi.ovmf_vars = Some(&vars);
        }
        uefi
    }
}

#[derive(Debug)]
pub struct QemuConfig<'a> {
    pub memory: &'a str,
    pub devices: &'a str,
    pub serial: Vec<&'a str>,
    pub kill_on_drop: bool,
    pub processor_count: u8,
    pub debug_log: Option<&'a Path>,
    pub debug_info: &'a str,
    pub uefi: UefiConfig<'a>,
}

impl<'a> QemuConfig<'a> {
    pub fn from_options(args: &'a QemuOptions) -> Self {
        Self {
            memory: "4G",
            devices: "",
            serial: vec!["stdio"],
            kill_on_drop: true,
            processor_count: args.processor_count,
            debug_log: args.qemu_log.as_ref().map(|p| p.as_path()),
            debug_info: args.qemu_info.as_str(),
            uefi: (&args.uefi).into(),
        }
    }
}

impl<'a> QemuConfig<'a> {
    pub fn add_serial(&mut self, serial: &'a str) -> Result<()> {
        if self.serial.len() >= 4 {
            bail!("Qemu only supports 4 serial ports");
        }

        self.serial.push(serial);
        Ok(())
    }
}

pub async fn launch_with_timeout<'a>(
    timeout: Duration,
    kernel: &Kernel<'a>,
    qemu: &QemuConfig<'a>,
) -> Result<ExitStatus> {
    let (mut child, _keep_alive) = launch_qemu(kernel, qemu).await?;

    match time::timeout(timeout, child.wait()).await {
        Ok(result) => result.context("qemu execution"),
        Err(_) => bail!("qemu execution timed out after {timeout:?}"),
    }
}

/// A placeholder Trait to defer the drop call of an object
pub trait AnyDrop {}
impl<T> AnyDrop for T {}

pub async fn launch_qemu<'a>(
    kernel: &Kernel<'a>,
    qemu: &QemuConfig<'a>,
) -> Result<(Child, Box<dyn AnyDrop>)> {
    let host_arch = HostArchitecture::get().await;

    let mut ovmf_paths = OvmfPaths::find(qemu).context("Load ovmf prebuild binaries for uefi")?;
    ovmf_paths
        .with_temp_vars()
        .context("make ovmf_vars a writable temp file")?;

    let mut cmd = Command::new(host_arch.qemu(Arch::X86_64).await);
    cmd.arg("-drive")
        .arg(&format!(
            "if=pflash,format=raw,readonly=on,file={}",
            ovmf_paths.code().display()
        ))
        .arg("-drive")
        .arg(&format!(
            "if=pflash,format=raw,readonly=off,file={}",
            ovmf_paths.vars().display()
        ));
    cmd.arg("-drive").arg(concat(
        "format=raw,id=boot,if=none,file=",
        host_arch.resolve(kernel.path).await,
    ));
    cmd.arg("-device").arg("nvme,serial=deadbeef,drive=boot");

    if qemu.serial.is_empty() {
        cmd.arg("-serial").arg("none");
    } else {
        for serial in qemu.serial.iter() {
            cmd.arg("-serial").arg(serial);
        }
    }

    // TODO temp nvme
    //  we use this device in integration tests and should find a way to keep it
    //  for tests. I still want to get rid of this for the normal execution environment
    cmd.arg("-drive")
        .arg("file=test_nvme_data.txt,if=none,id=nvm_test,format=raw");
    cmd.arg("-device")
        .arg("nvme,serial=deadbeef,drive=nvm_test");

    if qemu.debug_log.is_none() {
        if host_arch.is_windows() {
            cmd.arg("-accel").arg("whpx,kernel-irqchip=off");
            cmd.arg("-cpu").arg("max,vmx=off");
        } else {
            cmd.arg("-enable-kvm");
            cmd.arg("-cpu").arg("host");
        }
    }

    cmd.arg("-m").arg(qemu.memory);

    cmd.arg("-smp").arg(qemu.processor_count.to_string());

    if !qemu.devices.is_empty() {
        cmd.arg("-device").arg(qemu.devices);
    }

    if let Some(log_path) = qemu.debug_log {
        cmd.arg("-d").arg(qemu.debug_info);
        cmd.arg("-D").arg(log_path);
    }
    cmd.arg("-no-reboot");

    cmd.kill_on_drop(qemu.kill_on_drop);

    log::info!("{:?}", cmd);

    Ok((
        cmd.spawn().context("failed to spawn qemu")?,
        Box::new(ovmf_paths),
    ))
}

fn concat<A: AsRef<OsStr>, B: AsRef<OsStr>>(a: A, b: B) -> OsString {
    let a = a.as_ref();
    let b = b.as_ref();

    let mut result = OsString::with_capacity(a.len() + b.len());
    result.push(a);
    result.push(b);

    result
}

fn trim_u8_slice(string: &[u8]) -> &[u8] {
    let mut start = 0;
    loop {
        if string[start] != b' '
            && string[start] != b'\n'
            && string[start] != b'\t'
            && string[start] != b'\r'
        {
            break;
        }
        start += 1;
    }

    let mut end = string.len() - 1;
    loop {
        if string[end] != b' '
            && string[end] != b'\n'
            && string[end] != b'\t'
            && string[end] != b'\r'
        {
            break;
        }
        end -= 1;
    }

    &string[start..=end]
}

#[derive(Clone, Copy, Debug)]
pub enum HostArchitecture {
    Linux,
    Windows,
    Wsl,
}

impl HostArchitecture {
    pub async fn get() -> Self {
        if cfg!(windows) {
            return HostArchitecture::Windows;
        }

        if HostArchitecture::is_wsl().await {
            return HostArchitecture::Wsl;
        }

        return HostArchitecture::Linux;
    }

    pub async fn is_wsl() -> bool {
        let mut cmd = Command::new("wslpath");
        match cmd.output().await {
            Ok(_) => true,
            Err(_) => false,
        }
    }

    pub fn is_windows(&self) -> bool {
        match self {
            HostArchitecture::Windows | HostArchitecture::Wsl => true,
            _ => false,
        }
    }

    pub async fn resolve<T: Into<OsString>>(&self, path: T) -> OsString {
        match self {
            HostArchitecture::Linux | HostArchitecture::Windows => path.into(),
            HostArchitecture::Wsl => {
                let mut cmd = Command::new("wslpath");
                cmd.arg("-a").arg("-w").arg(path.into());

                let output = cmd
                    .output()
                    .await
                    .expect("could not execute \"wslpath\" command");

                if !output.status.success() {
                    panic!(
                        "Failed to execute \"wslpath\": {}\n{:?}",
                        output.status, output
                    );
                }
                OsStr::from_bytes(trim_u8_slice(&output.stdout)).into()
            }
        }
    }

    pub async fn resolve_back<T: Into<OsString>>(&self, path: T) -> OsString {
        match self {
            HostArchitecture::Linux | HostArchitecture::Windows => path.into(),
            HostArchitecture::Wsl => {
                let mut cmd = Command::new("wslpath");
                cmd.arg("--").arg(path.into());

                let output = cmd
                    .output()
                    .await
                    .expect("could not execute \"wslpath\" command");

                if !output.status.success() {
                    panic!(
                        "Failed to execute \"wslpath\": {}\n{:?}",
                        output.status, output
                    );
                }
                OsStr::from_bytes(trim_u8_slice(&output.stdout)).into()
            }
        }
    }

    pub async fn qemu(&self, arch: Arch) -> OsString {
        let mut qemu = OsString::new();

        let mut binary_suffix = if cfg!(windows) { ".exe" } else { "" };

        // path prefix
        match self {
            HostArchitecture::Wsl => {
                let mut cmd = Command::new("/mnt/c/Windows/System32/reg.exe");
                cmd.arg("query").arg("HKLM\\Software\\QEMU");
                cmd.arg("/v").arg("Install_Dir").arg("/t").arg("REG_SZ");

                let output = cmd
                    .output()
                    .await
                    .expect("could not execute \"reg.exe\" command");

                if !output.status.success() {
                    panic!(
                        "Failed to execute \"reg.exe\": {}\n{:?}",
                        output.status, output
                    );
                }

                let output = output.stdout;

                let install_dir_pos = output
                    .as_slice()
                    .windows(11)
                    .position(|w| OsStr::from_bytes(w) == "Install_Dir")
                    .expect("Install_Dir not found");

                // split after install_dir and move to start of actual data
                let (_, rest) = output.split_at(install_dir_pos + 11 + 14);

                let new_line_pos = rest
                    .windows(2)
                    .position(|w| OsStr::from_bytes(w) == "\r\n")
                    .expect("Expected new line after path");

                let (qemu_path, _) = rest.split_at(new_line_pos);

                qemu.push(OsStr::from_bytes(qemu_path));
                qemu.push("/");

                binary_suffix = ".exe";
            }
            _ => {}
        };
        qemu.push("qemu");

        qemu.push(arch.qemu_extension());

        qemu.push(binary_suffix);

        self.resolve_back(qemu).await
    }

    pub async fn host_ip_addr(&self) -> Result<Ipv4Addr> {
        match self {
            Self::Wsl => {
                let resolv_file = File::open("/etc/resolv.conf")
                    .await
                    .context("could not find \"/etc/resolv.conf\"")?;

                let mut lines = BufReader::new(resolv_file).lines();
                while let Some(line) = lines.next_line().await? {
                    if line.starts_with("nameserver ") {
                        let nameserver = &line[11..];
                        return Ok(nameserver
                            .parse()
                            .context("failed to parse localhost addr")?);
                    }
                }

                bail!("failed to find nameserver in resolv.conf");
            }
            _ => Ok(Ipv4Addr::new(127, 0, 0, 1)),
        }
    }
}
