use std::{
    ffi::{OsStr, OsString},
    os::unix::prelude::OsStrExt,
    path::Path,
    process::{Child, Command, ExitStatus},
    sync::mpsc::{self, RecvTimeoutError, TryRecvError},
    thread,
    time::Duration,
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

#[derive(Debug)]
pub struct Kernel<'a> {
    pub path: &'a Path,
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

pub fn launch_with_timeout<'a>(
    timeout: Duration,
    kernel: Kernel<'a>,
    qemu: QemuConfig<'a>,
) -> Result<ExitStatus, RecvTimeoutError> {
    let (sender, receiver) = mpsc::channel();
    let (kill_sender, kill_reciver) = mpsc::channel();

    let mut child = launch_qemu(kernel, qemu);
    let handle = thread::spawn(move || {
        loop {
            if let Ok(Some(exit)) = child.try_wait() {
                match sender.send(exit) {
                    Ok(_) => return,  // success
                    Err(_) => return, // reciever dropped, e.g timeout
                }
            } else {
                match kill_reciver.try_recv() {
                    Ok(_) | Err(TryRecvError::Disconnected) => {
                        let _ = child.kill();
                        return;
                    }
                    Err(TryRecvError::Empty) => {}
                }
                thread::yield_now();
            }
        }
    });

    match receiver.recv_timeout(timeout) {
        Ok(r) => Ok(r),
        Err(e) => {
            let _ = kill_sender.send(());
            let _ = handle.join();
            Err(e)
        }
    }
}

pub fn launch_qemu<'a>(kernel: Kernel<'a>, qemu: QemuConfig<'a>) -> Child {
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
    cmd.spawn().unwrap()
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
