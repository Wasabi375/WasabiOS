//! Module containig wrappers and utilities for cpu specific instructions
//! that are not dependent on the kernel

pub mod time;

/// Reads the instruction pointer.
///
/// Returns a [x86_64::VirtAddr] with the instruction pointer when this macro is called.
#[macro_export]
macro_rules! read_instruction_pointer {
    () => {
        {
            let ip: u64;
            // Safety just reading rip is save
            unsafe {
                asm!(
                    "lea {}, [rip]",
                    out (reg) ip,
                    options(nostack)
                );
            }
            x86_64::VirtAddr::new(ip)
        }
    };
}
