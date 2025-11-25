//! Syncronization primitives and functionality

pub mod thread;

use std::{
    cmp::max,
    collections::BTreeMap,
    num::NonZero,
    sync::{
        Mutex,
        atomic::{AtomicU8, Ordering},
    },
};

use shared::{
    sync::{CoreInfo, InterruptState},
    types::CoreId,
};

/// A std implementation for [CoreInfo] and [InterruptState]
pub struct StdInterruptState;

/// A static value of type [StdInterruptState]
///
/// This can be used to easily get access to a 'static reference.
pub static STD_INTERRUPT_STATE: StdInterruptState = StdInterruptState;

static THREAD_CORE_ID_MAP: Mutex<BTreeMap<NonZero<u64>, CoreId>> = Mutex::new(BTreeMap::new());

static NEXT_CORE_ID: AtomicU8 = AtomicU8::new(0);

fn next_core_id() -> CoreId {
    let id = NEXT_CORE_ID.fetch_add(1, Ordering::Relaxed);
    assert!(id < u8::MAX);
    CoreId(id)
}

impl CoreInfo for StdInterruptState {
    fn core_id(&self) -> CoreId {
        let thread_id = std::thread::current().id();
        *THREAD_CORE_ID_MAP
            .lock()
            .unwrap()
            .entry(thread_id.as_u64())
            .or_insert_with(next_core_id)
    }

    fn is_bsp(&self) -> bool {
        todo!("is_bsp")
    }

    fn is_initialized(&self) -> bool {
        true
    }

    fn instance() -> Self
    where
        Self: Sized,
    {
        StdInterruptState
    }

    fn max_core_count() -> u8
    where
        Self: Sized,
    {
        max(num_cpus::get(), u8::MAX as usize - 1) as u8
    }

    fn task_system_is_init(&self) -> bool {
        false
    }

    unsafe fn write_current_task_name(
        &self,
        _writer: &mut dyn core::fmt::Write,
    ) -> Result<(), core::fmt::Error> {
        Ok(())
    }
}

impl InterruptState for StdInterruptState {
    fn in_interrupt(&self) -> bool {
        false
    }

    fn in_exception(&self) -> bool {
        false
    }

    unsafe fn enter_lock(&self, _disable_interrupts: bool) {}

    unsafe fn exit_lock(&self, _enable_interrupts: bool) {}
}
