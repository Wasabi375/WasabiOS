//! PRP - Physical Region Page Entry

use core::mem::replace;

use alloc::vec::Vec;
use thiserror::Error;
use x86_64::{
    PhysAddr,
    structures::paging::{Page, PageSize, PhysFrame, Size4KiB},
};

use crate::mem::MemError;

/// The number of PRP entries that fit within a page
const PAGE_ENTRY_COUNT: usize = Size4KiB::SIZE as usize / size_of::<PrpEntry>();

/// A prp entry for the [CommonCommand](super::CommonCommand)
///
/// This can either be a list of prp entries or just a single entry
pub enum Prp {
    List(PrpList),
    Entry(PrpEntry),
    DoubleEntry(PrpEntry, PrpEntry),
}

impl Prp {
    /// The first prp entry for the [CommonCommand](super::CommonCommand)
    pub fn entry_1(&self) -> PrpEntry {
        match self {
            Prp::List(l) => l.first_entry,
            Prp::Entry(e) => *e,
            Prp::DoubleEntry(e1, _e2) => *e1,
        }
    }

    /// The second prp entry for the [CommonCommand](super::CommonCommand)
    pub fn entry_2(&self) -> PrpEntry {
        match self {
            Prp::List(l) => l.prp_list_frame,
            Prp::Entry(_) => PrpEntry::zero(),
            Prp::DoubleEntry(_e1, e2) => *e2,
        }
    }

    pub fn mappings(&self) -> &[(Page, PhysFrame)] {
        match self {
            Prp::List(l) => &l.mappings,
            Prp::Entry(_) | Prp::DoubleEntry(_, _) => &[],
        }
    }
}

/// A PrpList
///
/// See: NVM Express Base Specification: 4.1.1
pub struct PrpList {
    /// The first [PrpEntry]
    ///
    /// this entry is stored outside the list and passed as the first prp_entry in the
    /// [DataPtr](super::generic_command::DataPtr)
    pub first_entry: PrpEntry,
    /// The [PhysFrame] that contains the PrpList
    pub prp_list_frame: PrpEntry,
    /// all [Page]s and the [PhysFrame]s they are mapped to used by this list
    pub mappings: Vec<(Page<Size4KiB>, PhysFrame<Size4KiB>)>,
    /// the total number of entries within the list
    pub entry_count: usize,
}

impl PrpList {
    /// Constructs a [PrpList] from a list of [PrpEntries](PrpEntry).
    ///
    /// Only the first entry in `entries` can have an offset other than `0`. This
    /// will panic otherwise.
    ///
    /// # Safety:
    ///
    /// alloc_mapping must return a [Page], [PhysFrame] pair, where the page is mapped
    /// to write to the memory backed by the frame, without any existing references into the page
    pub unsafe fn new<Alloc>(
        entries: &[PrpEntry],
        mut alloc_mapping: Alloc,
    ) -> Result<PrpList, MemError>
    where
        Alloc: FnMut() -> Result<(Page<Size4KiB>, PhysFrame<Size4KiB>), MemError>,
    {
        assert!(
            entries.len() >= 3,
            "3 entries are required to form a list. Use first and second entry in DataPtr instead"
        );

        // TODO do I need to use volatile writes into the prp list?
        let mut mappings = Vec::new();

        let (page, frame) = alloc_mapping()?;
        mappings.try_reserve(1)?;
        mappings.push((page, frame));
        let prp_list_frame = PrpEntry::new(frame);

        let mut prp_page: &mut [PrpEntry; PAGE_ENTRY_COUNT] = unsafe {
            // Saftey: page is properly mapped
            &mut *page.start_address().as_mut_ptr()
        };
        let entry_count = entries.len();
        let mut entries_in_page = 0;

        let mut entries = entries.iter().copied();

        let first_entry = entries.next().expect("There should be at lest 3 entries");

        for entry in entries {
            assert!(
                entry.offset() == 0,
                "Only the first entry is allowed to have an offset"
            );

            if entries_in_page == PAGE_ENTRY_COUNT {
                let (page, frame) = alloc_mapping()?;
                mappings.try_reserve(1)?;
                mappings.push((page, frame));

                let last_page = prp_page;
                prp_page = unsafe {
                    // Saftey: page is properly mapped
                    &mut *page.start_address().as_mut_ptr()
                };

                // the last entry in the previous page should point to the phys addr
                // of the newly allocated page. Therefor we have to move the entry in
                // the last position into the first slot of this page.
                //
                // I don't just allocate the new page when only 1 slot is empty, because
                // if the total number of entries fits into a page, the last slot is
                // treated as a regular entry and not a next page pointer
                let last_entry =
                    replace(&mut last_page[PAGE_ENTRY_COUNT - 1], PrpEntry::new(frame));

                prp_page[0] = last_entry;
                entries_in_page = 1;
            }

            prp_page[entries_in_page] = entry;

            entries_in_page += 1;
        }

        Ok(Self {
            mappings,
            entry_count,
            first_entry,
            prp_list_frame,
        })
    }
}

/// A Prp Entry
///
/// See: NVM Express Base Specification: 4.1.1
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PrpEntry(u64);

#[derive(Error, Debug, PartialEq, Eq, Clone)]
#[allow(missing_docs)]
pub enum PrpEntryError {
    #[error("PrpEntries must be dword aligned")]
    NotDwordAligned,
    #[error("offset overflows page size")]
    OffsetToLarge,
}

impl PrpEntry {
    pub const fn new(frame: PhysFrame) -> PrpEntry {
        Self(frame.start_address().as_u64())
    }

    pub const fn zero() -> PrpEntry {
        PrpEntry::new(PhysFrame::containing_address(PhysAddr::zero()))
    }

    pub const fn with_offset(frame: PhysFrame, offset: u64) -> Result<Self, PrpEntryError> {
        if offset >= Size4KiB::SIZE {
            return Err(PrpEntryError::OffsetToLarge);
        }
        if offset & 0b11 != 0 {
            return Err(PrpEntryError::NotDwordAligned);
        }

        Ok(Self(frame.start_address().as_u64() | offset))
    }

    pub fn frame(&self) -> PhysFrame<Size4KiB> {
        PhysFrame::containing_address(PhysAddr::new(self.0))
    }

    pub fn offset(&self) -> u64 {
        self.0 & (Size4KiB::SIZE - 1)
    }

    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

impl core::fmt::Debug for PrpEntry {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PrpEntry")
            .field("frame", &self.frame())
            .field("offset", &self.offset())
            .finish()
    }
}

impl From<PhysFrame> for PrpEntry {
    fn from(value: PhysFrame) -> Self {
        Self::new(value)
    }
}

impl TryFrom<PhysAddr> for PrpEntry {
    type Error = PrpEntryError;

    fn try_from(value: PhysAddr) -> Result<Self, Self::Error> {
        let frame = PhysFrame::containing_address(value);
        let offset = value - frame.start_address();
        Self::with_offset(frame, offset)
    }
}
