#![no_std]

#[allow(unused_imports)]
extern crate base;

// build: cargo -Zjson-target-spec -Zbuild-std=core,alloc build --target ../kmodule-target.json
// symbols: objdump -s
// relocations: objdump -R

#[stabby::export(canaries)]
pub fn main() -> Result<(), u64> {
    Ok(())
}
