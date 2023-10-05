use crate::{
    args::TestArgs,
    qemu::{launch_with_timeout, Kernel, QemuConfig},
};
use anyhow::{bail, Context, Result};
use std::{path::Path, process::ExitStatus, time::Duration};

pub async fn test(uefi: &Path, mut args: TestArgs) -> Result<()> {
    let _kernel = Kernel {
        path: &uefi,
        uefi: true,
    };

    let mut qemu = QemuConfig {
        devices: "isa-debug-exit,iobase=0xf4,iosize=0x04",
        ..QemuConfig::default()
    };
    if args.tcp_port == "none" {
        args.keep_going = false;
        args.isolated = false;
        args.fast = true;
        return tests_no_tcp(uefi, args).await;
    } else {
        qemu.add_serial(&format!("tcp::{},server", args.tcp_port))?;
    }

    todo!("tcp - serial COM2 communication not implemented");
}

async fn tests_no_tcp(uefi: &Path, args: TestArgs) -> Result<()> {
    let kernel = Kernel {
        path: &uefi,
        uefi: true,
    };

    let qemu = QemuConfig {
        devices: "isa-debug-exit,iobase=0xf4,iosize=0x04",
        ..QemuConfig::default()
    };

    let exit_status = launch_with_timeout(Duration::from_secs(args.timeout), kernel, qemu)
        .await
        .context("test kernel")?;

    validate_qemu_run(exit_status)
}

fn validate_qemu_run(exit_status: ExitStatus) -> Result<()> {
    const SUCCESS: i32 = 0x10 << 1 | 1;
    const FAILURE: i32 = 0x11 << 1 | 1;

    if exit_status.success() {
        bail!("Qemu exit with code 0, but we expected {}", SUCCESS);
    } else {
        match exit_status.code() {
            Some(SUCCESS) => println!("Tests finished successfully"),
            Some(FAILURE) => bail!("Tests failed"),
            Some(code) => bail!("Qemu exited with unexpected exit code 0x{:x}", code),
            None => bail!("Qemu did not succeed, but has no error code????"),
        }
    }
    Ok(())
}
