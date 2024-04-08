//! Logic for starting Application Processors(APs)
//!
//! this is called by the Bootstrap Processor(BSP) after initialization
use core::sync::atomic::{AtomicU64, Ordering};

use shared::rangeset::{Range, RangeSet, RegionRequest};
use static_assertions::const_assert;
use x86_64::structures::paging::{PageSize, Size4KiB};

#[allow(unused)]
use log::{debug, error, info};

use crate::{mem::frame_allocator::PhysAllocator, todo_warn};

static AP_STARTUP_VECTOR: AtomicU64 = AtomicU64::new(0);

const BOOTSTRAP_CODE: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/src/cpu/apic/ap_trampoline.bin"));
const_assert!(BOOTSTRAP_CODE.len() as u64 <= Size4KiB::SIZE);

pub fn reserve_phys_frames(allocator: &mut PhysAllocator) {
    let mut boot_range = RangeSet::new();
    boot_range.insert(Range {
        start: 0x01000,
        end: 0xff000 + Size4KiB::SIZE,
    });

    let frame = allocator
        .alloc_in_range::<Size4KiB>(RegionRequest::Forced(&boot_range))
        .expect("Failed to allocate multiboot start frame");
    AP_STARTUP_VECTOR.store(frame.start_address().as_u64(), Ordering::Release);

    info!("Multiboot start frame: {frame:?}");
}

pub fn ap_startup() {
    info!("Starting APs");
    debug!("ap trampoline size: {}", BOOTSTRAP_CODE.len());
    todo_warn!()
}
