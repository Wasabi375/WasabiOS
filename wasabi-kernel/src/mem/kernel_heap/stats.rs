//! Stats about heap allocations

use alloc::boxed::Box;
use histogram::{Config, Histogram};
use log::{trace, warn};
use shared::math::IntoU64;

/// Statistics about the heap usage
#[derive(Clone)]
pub struct HeapStats {
    /// number of times there was no slab allocator for an allocation
    pub slab_misses: u64,
    /// bytes used within the heap
    pub used: u64,

    /// total number of bytes ever allocated.
    pub total_alloc: u64,
    /// total number of allocations
    pub alloc_count: u64,
    /// histogram of the allocation sizes
    pub alloc_sizes: Histogram,

    /// total number of bytes ever freed
    pub total_free: u64,
    /// total number of frees
    pub free_count: u64,
    /// histogram of the free sizes
    pub free_sizes: Histogram,
}

/// Statistics about the heap usage that is comparable
///
/// This only tracks current usage and does not include total counts
#[derive(Clone, PartialEq, Eq)]
pub struct HeapStatSnapshot {
    /// number of bytes used within the heap
    pub used: u64,
    /// number of allocations that have not been freed
    pub frees_outstanding: u64,
}

impl Default for HeapStats {
    fn default() -> Self {
        Self::new(7)
    }
}

impl HeapStats {
    /// Creates a new empty stats object.
    ///
    /// See [histogram::config::Config] for the `power` argument
    pub fn new(power: u8) -> Self {
        Self::new_with_powers(power, power)
    }

    /// Creates a new empty stats object.
    ///
    /// See [histogram::config::Config] for the `power` arguments
    pub fn new_with_powers(alloc_power: u8, free_power: u8) -> Self {
        let alloc_config = Config::new(alloc_power, 32).unwrap();
        let alloc_bucket_count = alloc_config.total_buckets();
        let alloc_buckets = unsafe { Box::new_zeroed_slice(alloc_bucket_count).assume_init() };
        let alloc_sizes = Histogram::from_buckets(alloc_power, 32, alloc_buckets).unwrap();

        let free_config = Config::new(free_power, 32).unwrap();
        let free_bucket_count = free_config.total_buckets();
        let free_buckets = unsafe { Box::new_zeroed_slice(free_bucket_count).assume_init() };
        let free_sizes = Histogram::from_buckets(free_power, 32, free_buckets).unwrap();

        HeapStats {
            alloc_sizes,
            free_sizes,
            used: 0,
            slab_misses: 0,
            total_alloc: 0,
            alloc_count: 0,
            total_free: 0,
            free_count: 0,
        }
    }

    /// Register an allocation
    pub fn register_alloc<U: IntoU64>(&mut self, bytes: U) {
        let bytes = bytes.into();
        trace!("register allock: {bytes}");

        self.used += bytes;
        self.total_alloc = self.total_alloc.wrapping_add(bytes);
        self.alloc_count += 1;

        match self.alloc_sizes.increment(bytes) {
            Ok(_) => {}
            Err(e) => warn!("Failed to register heap alloc size {e}"),
        }
    }

    /// Register that an allocation could not be fullfilled by a slab
    pub fn register_slab_miss(&mut self) {
        self.slab_misses += 1;
    }

    /// Register a free
    pub fn register_free<U: IntoU64>(&mut self, bytes: U) {
        let bytes = bytes.into();
        trace!("register free: {bytes}");

        assert!(self.used > bytes);
        self.used -= bytes;

        self.total_free = self.total_free.wrapping_add(bytes);
        self.free_count += 1;

        match self.free_sizes.increment(bytes) {
            Ok(_) => {}
            Err(e) => warn!("Failed to register heap free size {e}"),
        }
    }

    /// Creates a snapshot from the current stats
    pub fn snapshot(&self) -> HeapStatSnapshot {
        HeapStatSnapshot {
            used: self.used,
            frees_outstanding: self.alloc_count - self.free_count,
        }
    }

    /// Log the stats
    pub fn log(&self, level: log::Level) {
        log::log!(
            level,
            "Heap Stats: Alloc:\n\tused: {}\n\ttotal: {}\n\tcount: {}\n\tslab misses: {}",
            self.used,
            self.total_alloc,
            self.alloc_count,
            self.slab_misses,
        );
        for bucket in &self.alloc_sizes {
            if bucket.count() == 0 {
                continue;
            }
            log::log!(
                level,
                "Allocation size bucket {}..{}: {}",
                bucket.start(),
                bucket.end(),
                bucket.count()
            );
        }
        log::log!(
            level,
            "Heap Stats: Free:\n\ttotal: {}\n\tcount: {}",
            self.total_free,
            self.free_count
        );
        for bucket in &self.free_sizes {
            if bucket.count() == 0 {
                continue;
            }
            log::log!(
                level,
                "Free size bucket {}..{}: {}",
                bucket.start(),
                bucket.end(),
                bucket.count()
            );
        }
    }
}
