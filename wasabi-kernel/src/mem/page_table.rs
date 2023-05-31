#[allow(unused_imports)]
use log::{debug, error, info, trace, warn};

use crate::prelude::{LockCell, LockCellGuard, SpinLock};
use core::{fmt::Write, mem};
use lazy_static::lazy_static;
use shared::lockcell::LockCellInternal;
use staticvec::StaticString;
use x86_64::{
    structures::paging::{
        mapper::{MappedFrame, Translate, TranslateResult},
        page_table::FrameError,
        Page, PageSize, PageTable, PageTableFlags, PageTableIndex, PhysFrame, RecursivePageTable,
        Size1GiB, Size2MiB, Size4KiB,
    },
    PhysAddr, VirtAddr,
};

lazy_static! {
    static ref KERNEL_PAGE_TABLE: KernelPageTable = KernelPageTable::default();
}

pub struct KernelPageTable {
    table: SpinLock<Option<RecursivePageTable<'static>>>,
}

unsafe impl Send for KernelPageTable {}
unsafe impl Sync for KernelPageTable {}

impl Default for KernelPageTable {
    fn default() -> Self {
        Self {
            table: Default::default(),
        }
    }
}

impl KernelPageTable {
    pub fn get() -> &'static Self {
        &KERNEL_PAGE_TABLE
    }

    pub(super) fn init(&self, pt: RecursivePageTable<'static>) {
        *self.table.lock() = Some(pt);
    }
}

impl LockCellInternal<RecursivePageTable<'static>> for KernelPageTable {
    unsafe fn get(&self) -> &RecursivePageTable<'static> {
        self.table
            .get()
            .as_ref()
            .expect("Kernel page table needs to be initialized first")
    }

    unsafe fn get_mut(&self) -> &mut RecursivePageTable<'static> {
        self.table
            .get_mut()
            .as_mut()
            .expect("Kernel page table needs to be initialized first")
    }

    unsafe fn unlock<'s, 'l: 's>(
        &'s self,
        _guard: &mut LockCellGuard<'l, RecursivePageTable<'static>, Self>,
    ) {
        self.table.force_unlock();
    }

    unsafe fn force_unlock(&self) {
        self.table.force_unlock();
    }
}

impl LockCell<RecursivePageTable<'static>> for KernelPageTable {
    fn lock(&self) -> shared::lockcell::LockCellGuard<'_, RecursivePageTable<'static>, Self> {
        let guard = self.table.lock();
        // forgetting guard here is fine, we don't want to unlock the table
        // when we drop it here. Unlock is done by the new guard we return instead
        mem::forget(guard);
        LockCellGuard::new(self)
    }
}

pub trait RecursivePageTableExt {
    fn l4_table_vaddr(r: PageTableIndex) -> VirtAddr;
    fn l3_table_vaddr(r: PageTableIndex, l4_index: PageTableIndex) -> VirtAddr;
    fn l2_table_vaddr(
        r: PageTableIndex,
        l4_index: PageTableIndex,
        l3_index: PageTableIndex,
    ) -> VirtAddr;
    fn l1_table_vaddr(
        r: PageTableIndex,
        l4_index: PageTableIndex,
        l3_index: PageTableIndex,
        l2_index: PageTableIndex,
    ) -> VirtAddr;

    fn print_page_flags_for_vaddr(&mut self, vaddr: VirtAddr, message: Option<&str>);

    fn print_all_mapped_regions(&mut self, ignore_cpu_flags: bool);

    fn recursive_index(&mut self) -> PageTableIndex;
}

const L4_MASK: u64 = 0xff8000000000;

#[inline]
pub fn recursive_index() -> PageTableIndex {
    let boot_info = crate::boot_info();

    let index = *boot_info
        .recursive_index
        .as_ref()
        .expect("Expected boot info to contain recursive index");

    PageTableIndex::new(index)
}

impl<'a> RecursivePageTableExt for RecursivePageTable<'a> {
    #[inline]
    #[allow(dead_code)]
    fn recursive_index(&mut self) -> PageTableIndex {
        let vaddr = VirtAddr::new(self.level_4_table() as *const PageTable as u64);
        Page::<Size4KiB>::containing_address(vaddr).p4_index()
    }

    #[inline]
    #[allow(dead_code)]
    fn l4_table_vaddr(r: PageTableIndex) -> VirtAddr {
        let r: u64 = r.into();
        let vaddr = (r << 39) | (r << 30) | (r << 21) | (r << 12);

        VirtAddr::new(vaddr)
    }

    #[inline]
    #[allow(dead_code)]
    fn l3_table_vaddr(r: PageTableIndex, l4_index: PageTableIndex) -> VirtAddr {
        let r: u64 = r.into();
        let l4: u64 = l4_index.into();

        let vaddr = (r << 39) | (r << 30) | (r << 21) | (l4 << 12);

        VirtAddr::new(vaddr)
    }

    #[inline]
    #[allow(dead_code)]
    fn l2_table_vaddr(
        r: PageTableIndex,
        l4_index: PageTableIndex,
        l3_index: PageTableIndex,
    ) -> VirtAddr {
        let r: u64 = r.into();
        let l4: u64 = l4_index.into();
        let l3: u64 = l3_index.into();

        let vaddr = (r << 39) | (r << 30) | (l4 << 21) | (l3 << 12);

        VirtAddr::new(vaddr)
    }

    #[inline]
    #[allow(dead_code)]
    fn l1_table_vaddr(
        r: PageTableIndex,
        l4_index: PageTableIndex,
        l3_index: PageTableIndex,
        l2_index: PageTableIndex,
    ) -> VirtAddr {
        let r: u64 = r.into();
        let l4: u64 = l4_index.into();
        let l3: u64 = l3_index.into();
        let l2: u64 = l2_index.into();

        let vaddr = (r << 39) | (l4 << 30) | (l3 << 21) | (l2 << 12);

        VirtAddr::new(vaddr)
    }

    fn print_page_flags_for_vaddr(&mut self, vaddr: VirtAddr, message: Option<&str>) {
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
            info!(
                "{}: Frame flags for ptr({:p}) at page {}: Frame<{}>[{:p} - {:#x} - {:p}] {:?}",
                message, vaddr, page, size, start, offset, end, flags
            )
        } else {
            info!(
                "Frame flags for ptr({:p}) at page {}: Frame<{}>[{:p} - {:#x} - {:p}] {:?}",
                vaddr, page, size, start, offset, end, flags
            );
        }
    }

    /// Prints all mapped memory regions that are not part of the [BOOTLOADER_MEMORY_MAP_BASE]
    /// linear mapping of physical memory.
    fn print_all_mapped_regions(&mut self, ignore_cpu_flags: bool) {
        fn internal(
            level: u8,
            page_table: &PageTable,
            level_indices: [PageTableIndex; 4],
            last_region: &mut LinearMapMemRegion,
            ignore_cpu_flags: bool,
        ) {
            for (i, entry) in page_table.iter().enumerate() {
                if !entry.flags().contains(PageTableFlags::PRESENT) {
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
                    match entry.frame() {
                        Ok(frame) => {
                            let l4: u64 = level_indices[0].into();
                            let l3: u64 = level_indices[1].into();
                            let l2: u64 = level_indices[2].into();
                            let l1: u64 = level_indices[3].into();
                            let virt_addr = (l4 << 39) | (l3 << 30) | (l2 << 21) | (l1 << 12);

                            last_region.extend_or_print(
                                VirtAddr::new(virt_addr),
                                frame,
                                entry_flags,
                            )
                        }
                        Err(FrameError::FrameNotPresent) => {}
                        Err(other) => error!("unexpected error: {other:?}"),
                    }
                } else {
                    let l4_index: u64 = level_indices[0].into();
                    let l3_index: u64 = level_indices[1].into();
                    let l2_index: u64 = level_indices[2].into();
                    let l1_index: u64 = level_indices[3].into();
                    let entry_table_vaddr =
                        (l4_index << 39) | (l3_index << 30) | (l2_index << 21) | (l1_index << 12);

                    let entry_table: &PageTable =
                        unsafe { &*(entry_table_vaddr as *const PageTable) };
                    internal(
                        level - 1,
                        &entry_table,
                        level_indices,
                        last_region,
                        ignore_cpu_flags,
                    );
                }
            }
        }

        info!("Listing all frames for page table: ");
        let mut linear_mapping_mem_region = LinearMapMemRegion::empty();
        internal(
            4,
            self.level_4_table(),
            [recursive_index(); 4],
            &mut linear_mapping_mem_region,
            ignore_cpu_flags,
        );
        linear_mapping_mem_region.print();
    }
}

#[derive(Clone, Debug)]
struct LinearMapMemRegion {
    vstart: VirtAddr,
    vsize: u64,
    pstart: PhysAddr,
    psize: u64,
    flags: PageTableFlags,
}

impl LinearMapMemRegion {
    fn empty() -> Self {
        Self {
            vstart: VirtAddr::new(0),
            vsize: 0,
            pstart: PhysAddr::new(0),
            psize: 0,
            flags: PageTableFlags::empty(),
        }
    }

    fn extend_or_print<S: PageSize>(
        &mut self,
        vaddr: VirtAddr,
        frame: PhysFrame<S>,
        flags: PageTableFlags,
    ) {
        let vaddr_extended = vaddr == self.vstart + self.vsize;
        let paddr_extended = frame.start_address() == self.pstart + self.psize;
        let flags_match = self.flags == flags;

        if vaddr_extended && paddr_extended && flags_match {
            self.vsize += S::SIZE;
            self.psize += S::SIZE;
        } else {
            if self.vsize > 0 {
                self.print();
            }
            self.vstart = vaddr;
            self.vsize = S::SIZE;
            self.pstart = frame.start_address();
            self.psize = S::SIZE;
            self.flags = flags;
        }
    }

    fn print(&self) {
        let mut pt = StaticString::<20>::new();
        let r: u64 = recursive_index().into();
        if (self.vstart.as_u64() & L4_MASK) == r << 39 {
            let _ = write!(pt, "| PAGE_TABLE");
        }
        trace!(
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
