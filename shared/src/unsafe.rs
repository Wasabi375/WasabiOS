//! Unsafe utility functions with no better place to go

/// Extends the lifetime of a reference
///
/// # Safety:
///
/// Caller must ensure that the resulting lifetime is valid.
/// This function is UB in nearly all cases.
pub unsafe fn extend_lifetime<'i, 'o: 'i, T>(i: &'i T) -> &'o T {
    unsafe { &*(i as *const _) }
}

/// Extends the lifetime of a reference
///
/// # Safety:
///
/// Caller must ensure that the resulting lifetime is valid.
/// This function is UB in nearly all cases.
pub unsafe fn extend_lifetime_mut<'i, 'o: 'i, T>(i: &'i mut T) -> &'o mut T {
    unsafe { &mut *(i as *mut _) }
}
