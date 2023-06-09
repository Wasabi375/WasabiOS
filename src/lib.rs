use std::{
    ffi::{OsStr, OsString},
    io,
    os::unix::prelude::OsStrExt,
    process::{Command, ExitStatus},
};

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

#[derive(Debug, Default)]
pub struct Kernel<'a> {
    pub path: &'a str,
    pub uefi: bool,
}

#[derive(Debug)]
pub struct QemuConfig<'a> {
    pub memory: &'a str,
    pub devices: &'a str,
}

impl Default for QemuConfig<'_> {
    fn default() -> Self {
        Self {
            memory: "4G",
            devices: "",
        }
    }
}

pub fn launch_qemu<'a, F>(
    kernel: Kernel<'a>,
    qemu: QemuConfig<'a>,
    config: F,
) -> io::Result<ExitStatus>
where
    F: FnOnce(&mut Command, &HostArchitecture),
{
    let host_arch = HostArchitecture::get();

    let mut cmd = Command::new(host_arch.qemu(Arch::X86_64));
    if kernel.uefi {
        cmd.arg("-bios")
            .arg(host_arch.resolve(ovmf_prebuilt::ovmf_pure_efi()));
        cmd.arg("-drive")
            .arg(concat("format=raw,file=", host_arch.resolve(kernel.path)));
    } else {
        cmd.arg("-drive")
            .arg(concat("format=raw,file=", host_arch.resolve(kernel.path)));
    }
    cmd.arg("-serial").arg("stdio");

    if host_arch.is_windows() {
        cmd.arg("-accel").arg("whpx,kernel-irqchip=off");
        cmd.arg("-cpu").arg("max,vmx=off");
    } else {
        cmd.arg("-enable-kvm");
    }

    cmd.arg("-m").arg(qemu.memory);

    if !qemu.devices.is_empty() {
        cmd.arg("-device").arg(qemu.devices);
    }

    config(&mut cmd, &host_arch);

    let mut child = cmd.spawn().unwrap();
    child.wait()
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

pub enum HostArchitecture {
    Linux,
    Windows,
    Wsl,
}

impl HostArchitecture {
    pub fn get() -> Self {
        if cfg!(windows) {
            return HostArchitecture::Windows;
        }

        if HostArchitecture::is_wsl() {
            return HostArchitecture::Wsl;
        }

        return HostArchitecture::Linux;
    }

    pub fn is_wsl() -> bool {
        let mut cmd = Command::new("wslpath");
        match cmd.output() {
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

    pub fn resolve<T: Into<OsString>>(&self, path: T) -> OsString {
        match self {
            HostArchitecture::Linux | HostArchitecture::Windows => path.into(),
            HostArchitecture::Wsl => {
                let mut cmd = Command::new("wslpath");
                cmd.arg("-a").arg("-w").arg(path.into());

                let output = cmd.output().expect("could not execute \"wslpath\" command");

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

    pub fn resolve_back<T: Into<OsString>>(&self, path: T) -> OsString {
        match self {
            HostArchitecture::Linux | HostArchitecture::Windows => path.into(),
            HostArchitecture::Wsl => {
                let mut cmd = Command::new("wslpath");
                cmd.arg("--").arg(path.into());

                let output = cmd.output().expect("could not execute \"wslpath\" command");

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

    pub fn qemu(&self, arch: Arch) -> OsString {
        let mut qemu = OsString::new();

        let mut binary_suffix = if cfg!(windows) { ".exe" } else { "" };

        // path prefix
        match self {
            HostArchitecture::Wsl => {
                let mut cmd = Command::new("/mnt/c/Windows/System32/reg.exe");
                cmd.arg("query").arg("HKLM\\Software\\QEMU");
                cmd.arg("/v").arg("Install_Dir").arg("/t").arg("REG_SZ");

                let output = cmd.output().expect("could not execute \"reg.exe\" command");

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

        self.resolve_back(qemu)
    }
}
