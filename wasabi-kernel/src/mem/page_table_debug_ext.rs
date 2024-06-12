//! Debug extensions for the page table and paging

use core::{fmt::Write, mem::transmute_copy};
use log::{log, Level};
use staticvec::StaticString;
use x86_64::{
    structures::paging::{
        mapper::{MappedFrame, TranslateResult},
        page_table::{FrameError, PageTableEntry, PageTableLevel},
        Page, PageSize, PageTable, PageTableFlags, PageTableIndex, PhysFrame, RecursivePageTable,
        Size1GiB, Size2MiB, Size4KiB, Translate,
    },
    PhysAddr, VirtAddr,
};

#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

use crate::mem::page_table::{recursive_index, PageTableKernelFlags, RecursivePageTableExt};

/// Debug extensions for page tables
pub trait PageTableDebugExt {
    /// print the page table flags for the given vaddr
    fn print_page_flags_for_vaddr(
        &mut self,
        vaddr: VirtAddr,
        log_level: Level,
        message: Option<&str>,
    );

    /// print info for all table entries related to the given vaddr
    fn print_table_entries_for_vaddr(
        &mut self,
        vaddr: VirtAddr,
        log_level: Level,
        message: Option<&str>,
    );

    /// print all mapped memory regions
    fn print_all_mapped_regions(&mut self, ignore_cpu_flags: bool, log_level: Level);

    /// prints all entries of the page table
    fn print_page_table_for_vaddr(
        &mut self,
        vaddr: VirtAddr,
        level: PageTableLevel,
        log_level: Level,
        message: Option<&str>,
    );
}

impl<'a> PageTableDebugExt for RecursivePageTable<'a> {
    fn print_page_table_for_vaddr(
        &mut self,
        vaddr: VirtAddr,
        level: PageTableLevel,
        log_level: Level,
        message: Option<&str>,
    ) {
        let recursive = self.recursive_index();
        let table = match level {
            PageTableLevel::One => unsafe {
                &*Self::l1_table_vaddr(
                    recursive,
                    vaddr.p4_index(),
                    vaddr.p3_index(),
                    vaddr.p2_index(),
                )
                .as_ptr()
            },
            PageTableLevel::Two => unsafe {
                &*Self::l2_table_vaddr(recursive, vaddr.p4_index(), vaddr.p3_index()).as_ptr()
            },
            PageTableLevel::Three => unsafe {
                // this is a valid
                &*Self::l3_table_vaddr(recursive, vaddr.p4_index()).as_ptr()
            },
            PageTableLevel::Four => self.level_4_table(),
        };

        let TranslateResult::Mapped {
            frame,
            offset,
            flags,
        } = self.translate(VirtAddr::from_ptr(table))
        else {
            if let Some(message) = message {
                warn!("{}: PageTable not mapped!", message);
            } else {
                warn!("PageTable not mapped!");
            }
            return;
        };

        assert_eq!(offset, 0);

        if let Some(message) = message {
            log::log!(
                log_level,
                "{}: PageTable at {:p}: Frame: {:p}, flags: {:?}",
                message,
                table,
                frame.start_address(),
                flags
            );
        } else {
            log::log!(
                log_level,
                "PageTable at {:p}: Frame: {:p}, flags: {:?}",
                table,
                frame.start_address(),
                flags
            );
        }

        for (index, entry) in table.iter().enumerate() {
            if entry.is_unused() {
                continue;
            }

            let raw_bits: u64 = unsafe {
                // Safety: entry is repr transparent u64
                transmute_copy(entry)
            };

            log::log!(
                log_level,
                "[{index}]: PageTableEntry: {:p}, {:?} : {:#x}",
                entry.addr(),
                entry.flags(),
                raw_bits
            );
        }
    }

    fn print_page_flags_for_vaddr(&mut self, vaddr: VirtAddr, level: Level, message: Option<&str>) {
        let frame = self.translate(vaddr);
        let (frame, offset, flags) = match &frame {
            TranslateResult::Mapped {
                frame,
                offset,
                flags,
            } => (frame, offset, flags),
            _ => {
                if let Some(message) = message {
                    warn!("{message}: failed to find frame for virtual address {vaddr:p}");
                } else {
                    warn!("failed to find frame for virtual address {vaddr:p}");
                }
                return;
            }
        };
        let start: PhysAddr;
        let end: PhysAddr;
        let size: &str;
        let mut page: StaticString<30> = StaticString::new();
        match frame {
            MappedFrame::Size4KiB(frame) => {
                start = frame.start_address();
                end = frame.start_address() + frame.size();
                size = "4 KiB";
                write!(page, "{:?}", Page::<Size4KiB>::containing_address(vaddr))
            }
            MappedFrame::Size2MiB(frame) => {
                start = frame.start_address();
                end = frame.start_address() + frame.size();
                size = "2 MiB";
                write!(page, "{:?}", Page::<Size2MiB>::containing_address(vaddr))
            }
            MappedFrame::Size1GiB(frame) => {
                start = frame.start_address();
                end = frame.start_address() + frame.size();
                size = "1 GiB";
                write!(page, "{:?}", Page::<Size1GiB>::containing_address(vaddr))
            }
        }
        .expect("Debug write of Page failed into StaticString<30>");

        if let Some(message) = message {
            log!(
                level,
                "{}: Frame flags for ptr({:p}) at page {}: Frame<{}>[{:p} - {:#x} - {:p}] {:?}",
                message,
                vaddr,
                page,
                size,
                start,
                offset,
                end,
                flags
            )
        } else {
            log!(
                level,
                "Frame flags for ptr({:p}) at page {}: Frame<{}>[{:p} - {:#x} - {:p}] {:?}",
                vaddr,
                page,
                size,
                start,
                offset,
                end,
                flags
            );
        }
    }

    fn print_table_entries_for_vaddr(
        &mut self,
        vaddr: VirtAddr,
        level: Level,
        message: Option<&str>,
    ) {
        self.translate(vaddr);
        let recursive = self.recursive_index();
        if let Some(msg) = message {
            log!(level, "Page Table entries for {:p}: {}", vaddr, msg);
        } else {
            log!(level, "Page Table entries for {:p}", vaddr);
        }
        let l4_table = self.level_4_table();

        assert_eq!(
            l4_table as *const _ as u64,
            Self::l4_table_vaddr(recursive).as_u64()
        );

        let l3_table_vaddr = Self::l3_table_vaddr(recursive, vaddr.p4_index());
        let l2_table_vaddr = Self::l2_table_vaddr(recursive, vaddr.p4_index(), vaddr.p3_index());
        let l1_table_vaddr = Self::l1_table_vaddr(
            recursive,
            vaddr.p4_index(),
            vaddr.p3_index(),
            vaddr.p2_index(),
        );

        let l3_table = unsafe { &*l3_table_vaddr.as_ptr::<PageTable>() };
        let l2_table = unsafe { &*l2_table_vaddr.as_ptr::<PageTable>() };
        let l1_table = unsafe { &*l1_table_vaddr.as_ptr::<PageTable>() };

        let l4 = &l4_table[vaddr.p4_index()];
        let l3 = &l3_table[vaddr.p3_index()];
        let l2 = &l2_table[vaddr.p2_index()];
        let l1 = &l1_table[vaddr.p1_index()];

        fn print_table_entry(entry: &PageTableEntry, vaddr: VirtAddr, level: Level, msg: &str) {
            let frame = entry.frame().ok();
            let flags = entry.flags();
            let raw_bits: u64 = unsafe {
                // Safety: entry is repr transparent u64
                transmute_copy(entry)
            };

            match frame {
                Some(frame) => log!(
                    level,
                    "{}: Table: Vaddr {:p}:   Frame {:p} : Entry-Flags: {:?} : Raw-Entry {:#x}",
                    msg,
                    vaddr,
                    frame.start_address(),
                    flags,
                    raw_bits
                ),
                None => log!(
                    level,
                    "{}: Table: Vaddr {:p} : Entry-Flags: {:?} : Raw-Entry {:#x}",
                    msg,
                    vaddr,
                    flags,
                    raw_bits
                ),
            }
        }
        print_table_entry(l4, VirtAddr::from_ptr(l4_table), level, "L4 Entry");
        print_table_entry(l3, l3_table_vaddr, level, "L3 Entry");
        print_table_entry(l2, l2_table_vaddr, level, "L2 Entry");
        print_table_entry(l1, l1_table_vaddr, level, "L1 Entry");
    }

    fn print_all_mapped_regions(&mut self, ignore_cpu_flags: bool, log_level: Level) {
        fn internal(
            level: u8,
            page_table: &PageTable,
            level_indices: [PageTableIndex; 4],
            last_region: &mut LinearMapMemRegion,
            ignore_cpu_flags: bool,
            log_level: Level,
        ) {
            for (i, entry) in page_table.iter().enumerate() {
                if !entry
                    .flags()
                    .intersects(PageTableFlags::PRESENT | PageTableFlags::GUARD)
                {
                    continue;
                }

                let entry_flags = if ignore_cpu_flags {
                    entry.flags() & (PageTableFlags::ACCESSED | PageTableFlags::DIRTY).complement()
                } else {
                    entry.flags()
                };

                let mut level_indices = level_indices.clone();
                level_indices.rotate_left(1);
                level_indices[3] = PageTableIndex::new(i as u16);

                if entry_flags.contains(PageTableFlags::HUGE_PAGE) {
                    match level {
                        3 => {
                            let l4: u64 = level_indices[2].into();
                            let l3: u64 = level_indices[3].into();
                            let virt_addr = (l4 << 39) | (l3 << 30);

                            let frame =
                                PhysFrame::<Size1GiB>::from_start_address(entry.addr()).unwrap();

                            last_region.extend_or_print(
                                VirtAddr::new(virt_addr),
                                frame,
                                entry_flags,
                                log_level,
                            );
                        }
                        2 => {
                            let l4: u64 = level_indices[1].into();
                            let l3: u64 = level_indices[2].into();
                            let l2: u64 = level_indices[3].into();
                            let virt_addr = (l4 << 39) | (l3 << 30) | (l2 << 21);

                            let frame =
                                PhysFrame::<Size2MiB>::from_start_address(entry.addr()).unwrap();
                            last_region.extend_or_print(
                                VirtAddr::new(virt_addr),
                                frame,
                                entry_flags,
                                log_level,
                            );
                        }
                        1 => {
                            warn!("found huge frame in page table level {level}");
                            continue;
                        }
                        _ => continue,
                    };
                }

                if level == 1 {
                    let l4: u64 = level_indices[0].into();
                    let l3: u64 = level_indices[1].into();
                    let l2: u64 = level_indices[2].into();
                    let l1: u64 = level_indices[3].into();
                    let virt_addr = (l4 << 39) | (l3 << 30) | (l2 << 21) | (l1 << 12);

                    match entry.frame() {
                        Ok(frame) => last_region.extend_or_print(
                            VirtAddr::new(virt_addr),
                            frame,
                            entry_flags,
                            log_level,
                        ),
                        Err(FrameError::FrameNotPresent) => {
                            if entry_flags.intersects(PageTableFlags::PRESENT_OR_USED) {
                                let fake_frame: PhysFrame<Size4KiB> =
                                    PhysFrame::from_start_address(PhysAddr::new(0)).unwrap();
                                last_region.extend_or_print(
                                    VirtAddr::new(virt_addr),
                                    fake_frame,
                                    entry_flags,
                                    log_level,
                                )
                            }
                        }
                        Err(other) => error!("unexpected error: {other:?}"),
                    }
                } else {
                    let l4_index: u64 = level_indices[0].into();
                    let l3_index: u64 = level_indices[1].into();
                    let l2_index: u64 = level_indices[2].into();
                    let l1_index: u64 = level_indices[3].into();
                    let entry_table_vaddr =
                        (l4_index << 39) | (l3_index << 30) | (l2_index << 21) | (l1_index << 12);

                    // safety: entry_table_vaddr is a valid pointer to the next level page table
                    // in a recurcive page table
                    let entry_table: &PageTable =
                        unsafe { &*(entry_table_vaddr as *const PageTable) };
                    internal(
                        level - 1,
                        &entry_table,
                        level_indices,
                        last_region,
                        ignore_cpu_flags,
                        log_level,
                    );
                }
            }
        }

        log!(log_level, "Listing all frames for page table: ");
        let mut linear_mapping_mem_region = LinearMapMemRegion::empty();
        let recursive_index = self.recursive_index();
        internal(
            4,
            self.level_4_table(),
            [recursive_index; 4],
            &mut linear_mapping_mem_region,
            ignore_cpu_flags,
            log_level,
        );
        linear_mapping_mem_region.print(log_level);
    }
}

/// utility struct for [RecursivePageTable::print_page_flags_for_vaddr]
/// representing a linear region of memory, both in virt and phys space
#[derive(Clone, Debug)]
struct LinearMapMemRegion {
    vstart: VirtAddr,
    vsize: u64,
    pstart: PhysAddr,
    psize: u64,
    flags: PageTableFlags,
}

impl LinearMapMemRegion {
    /// creates a new empty [LinearMapMemRegion]
    fn empty() -> Self {
        Self {
            vstart: VirtAddr::new(0),
            vsize: 0,
            pstart: PhysAddr::new(0),
            psize: 0,
            flags: PageTableFlags::empty(),
        }
    }

    /// tries to extend the [LinearMapMemRegion].
    ///
    /// If the extension is not connected to self, [`print`](LinearMapMemRegion::print)s
    /// the region and modifies `self` to include only the extension.
    fn extend_or_print<S: PageSize>(
        &mut self,
        vaddr: VirtAddr,
        frame: PhysFrame<S>,
        flags: PageTableFlags,
        level: Level,
    ) {
        let vaddr_extended = vaddr == self.vstart + self.vsize;
        let paddr_extended = frame.start_address() == self.pstart + self.psize;
        let flags_match = self.flags == flags;

        if vaddr_extended && paddr_extended && flags_match {
            self.vsize += S::SIZE;
            self.psize += S::SIZE;
        } else {
            if self.vsize > 0 {
                self.print(level);
            }
            self.vstart = vaddr;
            self.vsize = S::SIZE;
            self.pstart = frame.start_address();
            self.psize = S::SIZE;
            self.flags = flags;
        }
    }

    /// prints [self]
    fn print(&self, level: Level) {
        const L4_MASK: u64 = 0xff8000000000;

        let mut pt = StaticString::<20>::new();
        let r: u64 = recursive_index().into();
        if (self.vstart.as_u64() & L4_MASK) == r << 39 {
            let _ = write!(pt, "| PAGE_TABLE");
        }
        log!(
            level,
            "Vaddr {:p} - {:p}:   Frame {:p} - {:p} | {:?} {}",
            self.vstart,
            self.vstart + self.vsize,
            self.pstart,
            self.pstart + self.psize,
            self.flags,
            pt
        );
    }
}
