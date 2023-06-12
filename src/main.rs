extern crate wasabi_os;
use wasabi_os::{launch_qemu, Kernel, QemuConfig};

fn main() {
    let kernel_path = env!("KERNEL_PATH");
    println!("Kernel elf at: {kernel_path}");

    // read env variables that were set in build script
    let uefi_path = env!("UEFI_PATH");

    let kernel = Kernel {
        path: uefi_path,
        uefi: true,
    };

    let qemu = QemuConfig::default();

    launch_qemu(kernel, qemu, |_cmd, _host_arch| {}).unwrap();
}
