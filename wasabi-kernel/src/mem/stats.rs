//! Stats about memory usage

use alloc::boxed::Box;
use histogram::{Config, Histogram};
use log::{trace, warn};
use shared::math::IntoU64;
use x86_64::structures::paging::{PageSize, PageTableFlags, Size1GiB, Size2MiB, Size4KiB};

use crate::mem::page_table::PageTableKernelFlags;

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

        assert!(self.used >= bytes, "{} >= {}", self.used, bytes);
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

/// Statistics about page allocations
#[derive(Clone, Default, Debug)]
pub struct PageFrameAllocStats {
    /// Count of allocated 4k sized pages
    pub alloc_4k_count: u64,
    /// Count of allocated 2m allocd pages
    pub alloc_2m_count: u64,
    /// Count of allocated 1g allocd pages
    pub alloc_1g_count: u64,

    /// Count of freed 4k sized pages
    pub free_4k_count: u64,
    /// Count of freed 2m allocd pages
    pub free_2m_count: u64,
    /// Count of freed 1g allocd pages
    pub free_1g_count: u64,
}

/// Statistics about the page allocations that is comparable
///
/// This only tracks current usage and does not include total counts
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PageAllocStatSnapshot {
    /// number of currently allocated pages with size 4k
    pub count_4k: u64,
    /// number of currently allocated pages with size 2m
    pub count_2m: u64,
    /// number of currently allocated pages with size 1g
    pub count_1g: u64,
}

impl PageFrameAllocStats {
    /// Register an allocation
    pub fn register_alloc<S: PageSize>(&mut self, count: u64) {
        match S::SIZE {
            Size4KiB::SIZE => {
                self.alloc_4k_count += count;
            }
            Size2MiB::SIZE => {
                self.alloc_2m_count += count;
            }
            Size1GiB::SIZE => {
                self.alloc_1g_count += count;
            }
            _ => unreachable!("Page::SIZE not supported"),
        }
    }

    /// Register a free
    pub fn register_free<S: PageSize>(&mut self, count: u64) {
        match S::SIZE {
            Size4KiB::SIZE => {
                self.free_4k_count += count;
            }
            Size2MiB::SIZE => {
                self.free_2m_count += count;
            }
            Size1GiB::SIZE => {
                self.free_1g_count += count;
            }
            _ => unreachable!("Page::SIZE not supported"),
        }
    }

    /// Creates a snapshot from the current stats
    pub fn snapshot(&self) -> PageAllocStatSnapshot {
        PageAllocStatSnapshot {
            count_4k: self.alloc_4k_count - self.free_4k_count,
            count_2m: self.alloc_2m_count - self.free_2m_count,
            count_1g: self.alloc_1g_count - self.free_1g_count,
        }
    }

    /// Log the stats
    pub fn log(&self, level: log::Level, msg: Option<&str>) {
        if let Some(msg) = msg {
            log::log!(level, "{msg}: {:#?}", self);
        } else {
            log::log!(level, "{:#?}", self);
        }
    }
}

/// Statistics about mappings within a page table
#[derive(Clone, Debug)]
pub struct PageTableStats {
    /// the total number of 4k pages mapped
    pub total_mapped_4k: u64,
    /// the total number of 2m pages mapped
    pub total_mapped_2m: u64,
    /// the total number of 1g pages mapped
    pub total_mapped_1g: u64,

    /// the total number of 4k pages unmapped
    pub total_unmapped_4k: u64,
    /// the total number of 2m pages unmapped
    pub total_unmapped_2m: u64,
    /// the total number of 1g pages unmapped
    pub total_unmapped_1g: u64,

    /// the total number of 4k pages remapped
    pub total_remapped_4k: u64,
    /// the total number of 2m pages remapped
    pub total_remapped_2m: u64,
    /// the total number of 1g pages remapped
    pub total_remapped_1g: u64,

    /// the total number of pages mapped without the present flag
    pub total_mapped_without_present: u64,

    /// total count of pages mapped by [PageTableFlags] based on
    /// the bit offset going low to high
    pub total_mapping_by_flag: [u64; 64],
}

impl Default for PageTableStats {
    fn default() -> Self {
        Self {
            total_mapped_4k: 0,
            total_mapped_2m: 0,
            total_mapped_1g: 0,
            total_unmapped_4k: 0,
            total_unmapped_2m: 0,
            total_unmapped_1g: 0,
            total_remapped_4k: 0,
            total_remapped_2m: 0,
            total_remapped_1g: 0,
            total_mapped_without_present: 0,
            total_mapping_by_flag: [0; 64],
        }
    }
}

impl PageTableStats {
    /// register the flags used in a map or remap
    fn register_flags(&mut self, flags: PageTableFlags) {
        if !flags.contains(PageTableFlags::PRESENT) {
            self.total_mapped_without_present += 1;
        }

        for i in 0..64 {
            let bit = 1 << i;
            let Some(f) = PageTableFlags::from_bits(bit) else {
                continue;
            };
            if flags.contains(f) {
                self.total_mapping_by_flag[i] += 1;
            }
        }
    }

    /// register that a page was mapped
    pub fn register_map<S: PageSize>(&mut self, flags: PageTableFlags) {
        match S::SIZE {
            Size4KiB::SIZE => {
                self.total_mapped_4k += 1;
            }
            Size2MiB::SIZE => {
                self.total_mapped_2m += 1;
            }
            Size1GiB::SIZE => {
                self.total_mapped_1g += 1;
            }
            _ => unreachable!("Page::SIZE not supported"),
        }
        self.register_flags(flags)
    }

    /// register that a page was remapped
    pub fn register_remap<S: PageSize>(&mut self, flags: PageTableFlags) {
        match S::SIZE {
            Size4KiB::SIZE => {
                self.total_remapped_4k += 1;
            }
            Size2MiB::SIZE => {
                self.total_remapped_2m += 1;
            }
            Size1GiB::SIZE => {
                self.total_remapped_1g += 1;
            }
            _ => unreachable!("Page::SIZE not supported"),
        }
        self.register_flags(flags)
    }

    /// register that a page was unmapped
    pub fn register_unmap<S: PageSize>(&mut self, _flags: PageTableFlags) {
        match S::SIZE {
            Size4KiB::SIZE => {
                self.total_unmapped_4k += 1;
            }
            Size2MiB::SIZE => {
                self.total_unmapped_2m += 1;
            }
            Size1GiB::SIZE => {
                self.total_unmapped_1g += 1;
            }
            _ => unreachable!("Page::SIZE not supported"),
        }
    }

    /// Creates a snapshot from the current stats
    pub fn snapshot(&self) -> PagetTableStatSnapshot {
        PagetTableStatSnapshot {
            count_4k: self.total_mapped_4k - self.total_unmapped_4k,
            count_2m: self.total_mapped_2m - self.total_unmapped_2m,
            count_1g: self.total_mapped_1g - self.total_unmapped_1g,
        }
    }

    /// Log the stats
    pub fn log(&self, level: log::Level) {
        log::log!(level, "PageTableStats:\n\tmapped: {} 4k, {} 2m, {} 1g\n\tunmapped: {} 4k, {} 2m, {} 1g\n\tremapped: {} 4k, {} 2m, {} 1g\n\tnot present: {}",
            self.total_mapped_4k, self.total_mapped_2m, self.total_mapped_1g,
            self.total_unmapped_4k, self.total_unmapped_2m, self.total_unmapped_1g,
            self.total_remapped_4k, self.total_remapped_2m, self.total_remapped_1g,
            self.total_mapped_without_present);
        for i in 0..64 {
            let Some(flag) = PageTableFlags::from_bits(1 << i) else {
                continue;
            };
            let count = self.total_mapping_by_flag[i];
            if count != 0 {
                log::log!(level, "{} count: {}", flag.get_name(), count);
            }
        }
    }
}

/// Statistics about mappings in a page table that is comparable
///
/// This only tracks current usage and does not include total counts
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PagetTableStatSnapshot {
    /// total number of 4k pages currently mapped
    pub count_4k: u64,
    /// total number of 2m pages currently mapped
    pub count_2m: u64,
    /// total number of 1g pages currently mapped
    pub count_1g: u64,
}
