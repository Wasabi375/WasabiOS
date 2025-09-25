//! Module containing cpu instructions related to time and timing
//!
use core::arch::x86_64::{_mm_lfence, _mm_mfence, _rdtsc};

use crate::types::TscTimestamp;

/// read the time stamp counter
///
/// The RDTSC instruction is not a  serializing instruction. It does not
/// necessarily wait until all previous instructions have been executed before
/// reading the counter. Similarly, subsequent instructions may begin execution
/// before the read operation is performed.
///
/// Use [read_tsc_fenced] if instruction order is important, however this
/// is less performant.
#[inline(always)]
pub fn timestamp_now_tsc() -> TscTimestamp {
    unsafe { _rdtsc() }.into()
}

/// read the time stamp counter
///
/// This also ensures that all previous stores and loads are globaly visible
/// before reading tsc. And that tsc is read, before any following operation
/// is executed.  
/// If this is not strictly necessray [read_tsc] should be used instead as it
// allows for better performance.
///
/// This is the same as
/// ```no_run
/// # use std::arch::x86_64::_mm_lfence;
/// # use std::arch::x86_64::_rdtsc;
/// # unsafe {
/// _mm_lfence();
// _mm_mfence();
/// let tsc: u64 = _rdtsc() ;
/// _mm_lfence();
/// # }
/// ```
///
/// See [_mm_lfence], [_mm_mfence]
#[inline(always)]
pub fn timestamp_now_tsc_fenced() -> TscTimestamp {
    unsafe {
        _mm_lfence();
        _mm_mfence();
        let tsc = timestamp_now_tsc();
        _mm_lfence();
        tsc
    }
}
