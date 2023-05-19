use std::{
    ffi::{OsStr, OsString},
    os::unix::prelude::{OsStrExt, OsStringExt},
    path::PathBuf,
    process::Command,
};

enum Arch {
    X86_64,
}

impl Arch {
    fn qemu_extension(&self) -> &'static str {
        match self {
            Arch::X86_64 => "-system-x86_64",
        }
    }
}

fn main() {
    // TODO resolve wsl paths
    //
    let path_resolver = PathResolver::get();

    // read env variables that were set in build script
    let uefi_path = path_resolver.resolve(env!("UEFI_PATH"));
    let bios_path = path_resolver.resolve(env!("BIOS_PATH"));

    // choose whether to start the UEFI or BIOS image
    let uefi = true;

    let mut cmd = Command::new(path_resolver.qemu(Arch::X86_64));
    if uefi {
        cmd.arg("-bios")
            .arg(path_resolver.resolve(ovmf_prebuilt::ovmf_pure_efi()));
        cmd.arg("-drive").arg(concat("format=raw,file=", uefi_path));
    } else {
        cmd.arg("-drive").arg(concat("format=raw,file=", bios_path));
    }
    let mut child = cmd.spawn().unwrap();
    child.wait().unwrap();
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

enum PathResolver {
    Identity,
    Wsl,
}

impl PathResolver {
    fn get() -> Self {
        if cfg!(windows) {
            return PathResolver::Identity;
        }

        if PathResolver::is_wsl() {
            return PathResolver::Wsl;
        }

        return PathResolver::Identity;
    }

    fn is_wsl() -> bool {
        let mut cmd = Command::new("wslpath");
        match cmd.output() {
            Ok(_) => true,
            Err(_) => false,
        }
    }

    fn resolve<T: Into<OsString>>(&self, path: T) -> OsString {
        match self {
            PathResolver::Identity => path.into(),
            PathResolver::Wsl => {
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

    fn resolve_back<T: Into<OsString>>(&self, path: T) -> OsString {
        match self {
            PathResolver::Identity => path.into(),
            PathResolver::Wsl => {
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

    fn qemu(&self, arch: Arch) -> OsString {
        let mut qemu = OsString::new();

        let mut binary_suffix = if cfg!(windows) { ".exe" } else { "" };

        // path prefix
        match self {
            PathResolver::Identity => {}
            PathResolver::Wsl => {
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
        };
        qemu.push("qemu");

        qemu.push(arch.qemu_extension());

        qemu.push(binary_suffix);

        self.resolve_back(qemu)
    }
}
