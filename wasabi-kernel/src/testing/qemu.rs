//! Qemu utilities

/// Exit code for qemu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ExitCode {
    /// kernel exit successfuly.
    Success = 0x10,
    /// an error occured
    Error = 0x11,
}

/// stop the kernel and exit qeum
pub fn exit(code: ExitCode) -> ! {
    use x86_64::instructions::port::Port;

    unsafe {
        let mut port = Port::new(0xf4);
        port.write(code as u32);
    }
    unreachable!();
}
