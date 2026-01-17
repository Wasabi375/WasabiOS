#![no_std]
#![allow(incomplete_features)]
#![feature(raw_dylib_elf)]

pub mod global_alloc;
pub mod panic;

#[stabby::import(name = "kernel-core", canaries = "", kind = "raw-dylib")]
#[allow(improper_ctypes)]
extern "C" {
    pub fn serial_print(message: &str);
    pub fn exit() -> !;
}

#[stabby::export(canaries)]
pub extern "C" fn __kmodule_main() {
    unsafe {
        serial_print("test");
        exit();
    }
}
