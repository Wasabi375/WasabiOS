use std::{env::var_os, path::PathBuf};

fn main() {
    // set by cargo, build scripts should use this directory for output files
    let out_dir = PathBuf::from(var_os("OUT_DIR").unwrap());
    // set by cargo's artifact dependency feature, see
    // https://doc.rust-lang.org/nightly/cargo/reference/unstable.html#artifact-dependencies
    let kernel = PathBuf::from(var_os("CARGO_BIN_FILE_WASABI_KERNEL").unwrap());

    // create an UEFI disk image (optional)
    let uefi_path = out_dir.join("uefi.img");
    bootloader::UefiBoot::new(&kernel)
        .create_disk_image(&uefi_path)
        .unwrap();

    // create a BIOS disk image
    #[cfg(feature = "bios-boot")]
    {
        let bios_path = out_dir.join("bios.img");
        bootloader::BiosBoot::new(&kernel)
            .create_disk_image(&bios_path)
            .unwrap();
    }

    build_test_kernel(out_dir);

    // pass the disk image paths as env variables to the `main.rs`
    println!("cargo:rustc-env=UEFI_PATH={}", uefi_path.display());

    #[cfg(feature = "bios-boot")]
    println!("cargo:rustc-env=BIOS_PATH={}", bios_path.display());

    println!("cargo:rustc-env=KERNEL_PATH={}", kernel.display());
}

fn build_test_kernel(out_dir: PathBuf) {
    let test_kernel = PathBuf::from(var_os("CARGO_BIN_FILE_WASABI_TEST").unwrap());

    // create an UEFI disk image (optional)
    let uefi_path = out_dir.join("test_kernel_uefi.img");
    bootloader::UefiBoot::new(&test_kernel)
        .create_disk_image(&uefi_path)
        .unwrap();

    println!(
        "cargo:rustc-env=TEST_KERNEL_UEFI_PATH={}",
        uefi_path.display()
    );
}
