extern crate wasabi_os;

use wasabi_os::{launch_qemu, Kernel, QemuConfig};

const SUCCESS: i32 = 0x10 << 1 | 1;
const FAILURE: i32 = 0x11 << 1 | 1;

#[test]
fn qemu_test_kernel() {
    // read env variables that were set in build script
    let uefi_path = env!("TEST_KERNEL_UEFI_PATH");

    let kernel = Kernel {
        path: uefi_path,
        uefi: true,
    };

    let qemu = QemuConfig {
        devices: "isa-debug-exit,iobase=0xf4,iosize=0x04",
        ..QemuConfig::default()
    };
    match launch_qemu(kernel, qemu, |_cmd, _host_arch| {}) {
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
