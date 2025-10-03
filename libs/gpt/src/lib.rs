//! Structs representing different UEFI GPT records
//!
//! The specifications are available from <https://uefi.org/specifications>
//! Specifically relevant is the UEFI Specification.
//!
//! This is based on UEFI Specifiaction Version 2.10(released Auguast 2022),
//! although I assume that any 2.x version works
#![allow(incomplete_features)]
#![feature(generic_const_exprs, allocator_api, slice_as_array)]
#![no_std]

extern crate alloc;

use core::{
    error::Error,
    hash::{Hash, Hasher},
    mem::MaybeUninit,
    num::NonZeroU64,
};

use alloc::boxed::Box;
use block_device::{LBA, ReadBlockDeviceError, SizedBlockDevice};
use protective_mbr::ProtectiveMBR;
use shared::{
    alloc_ext::{AllocError, alloc_buffer_aligned},
    counts_required_for,
};
use thiserror::Error;

pub mod chs;

pub mod protective_mbr;

#[derive(Debug, Error, Clone, Copy)]
#[allow(missing_docs)]
pub enum InvalidHeader {
    #[error("The signature must match \"EFI PART\"")]
    InvalidSignature,
    #[error("Expected header to have crc {0} but calculated {1}")]
    InvalidHeaderCrc(u32, u32),
    #[error("Expected my_lba to be {1} but found {0}")]
    InvalidMyLBA(u64, u64),
    #[error("Header({0} bytes) does not fit within a block ({1} bytes)")]
    InvalidHeaderSize(u32, u64),
    #[error("Invalid partition entry size {0}")]
    InvalidPartitionEntrySize(u32),
    #[error("LBA {0} is invalid")]
    InvalidLBA(u64),
    #[error("Expected entry array to have crc {0} but calculated {1}")]
    InvalidEntryArrayCrc(u32, u32),
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
#[repr(C)]
#[allow(missing_docs)]
/// a GPT Header
///
/// See UEFI spec 5.3.2
pub struct Header {
    /// should always be "EFI PART"
    pub signature: [u8; 8],
    /// The version of the header implementation
    ///
    /// This should be 1.0 `0x0001_0000`. This is *not* the spec version.
    pub revision: u32,

    pub header_size: u32,
    pub header_crc32: u32,

    /// must be 0
    pub reserved: u32,

    pub my_lba: u64,
    pub alternate_lba: u64,

    pub disk_guid: [u8; 16],

    pub partition_entry_lba: u64,
    pub number_of_partitions: u32,
    pub size_of_partition_entry: u32,

    pub partition_entry_array_crc32: u32,
}

impl Header {
    /// Calculate the crc32 for the header
    ///
    /// this ignores any value already stored in [Self::header_crc32]
    pub fn calculate_crc32(&self) -> u32 {
        let mut hasher = crc32fast::Hasher::new();

        hasher.write(&self.signature);
        hasher.write_u32(self.revision);
        hasher.write_u32(self.header_size);
        hasher.write_u32(self.reserved);
        hasher.write_u64(self.my_lba);
        hasher.write_u64(self.alternate_lba);
        hasher.write(&self.disk_guid);
        hasher.write_u64(self.partition_entry_lba);
        hasher.write_u32(self.number_of_partitions);
        hasher.write_u32(self.size_of_partition_entry);
        hasher.write_u32(self.partition_entry_array_crc32);

        hasher.finalize()
    }

    /// Calculate the crc32 for the header and set [Self::header_crc32]
    pub fn calc_and_set_crc32(&mut self) {
        self.header_crc32 = self.calculate_crc32();
    }

    /// Verify that the header valid
    ///
    /// This does not check that the partition array is valid
    pub fn verify(&self, header_lba: u64) -> Result<(), InvalidHeader> {
        if self.signature != *b"EFI PART" {
            return Err(InvalidHeader::InvalidSignature);
        }
        let calculated_crc32 = self.calculate_crc32();
        if self.header_crc32 != calculated_crc32 {
            return Err(InvalidHeader::InvalidHeaderCrc(
                self.header_crc32,
                calculated_crc32,
            ));
        }
        if self.my_lba != header_lba {
            return Err(InvalidHeader::InvalidMyLBA(self.my_lba, header_lba));
        }

        if self.size_of_partition_entry % 128 != 0 {
            return Err(InvalidHeader::InvalidPartitionEntrySize(
                self.size_of_partition_entry,
            ));
        }

        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq, Hash)]
#[repr(C)]
#[allow(missing_docs)]
/// A partition entry for a GPT
///
/// See UEFI spec 5.3.3
pub struct PartitionEntry {
    partition_type_guid: [u8; 16],
    partition_guid: [u8; 16],
    starting_lba: u64,
    /// ending lba of partition (inclusive)
    endina_lba: u64,

    attributes: u64,

    partition_name: [u8; 72],
}

impl PartitionEntry {
    pub fn calculate_array_crc(entries: &[PartitionEntry]) -> u32 {
        let mut hasher = crc32fast::Hasher::new();
        for entry in entries {
            entry.hash(&mut hasher);
        }
        hasher.finalize()
    }
}

pub struct GPT {
    pub warn_invalid_mbr: Option<protective_mbr::InvalidMBRData>,
    pub warn_invalid_primary_header: Option<InvalidHeader>,
    pub partitions: Box<[PartitionEntry]>,
    pub header: Header,
}

#[derive(thiserror::Error, Debug)]
#[allow(missing_docs)]
pub enum GPTReadError<BDError: Error + Send + Sync + 'static> {
    #[error("Block device error: {0}")]
    ReadBlockDevice(#[from] ReadBlockDeviceError<BDError>),
    #[error("Failed to allocate memory(RAM): {0}")]
    Allocation(#[from] AllocError),
    #[error("both primary and backup headers are invalid! Primary: {0}, Backup: {1}")]
    InvalidHeaders(InvalidHeader, InvalidHeader),
}

impl<BDError: Error + Send + Sync + 'static> From<core::alloc::AllocError>
    for GPTReadError<BDError>
{
    fn from(value: core::alloc::AllocError) -> Self {
        Self::Allocation(value.into())
    }
}

pub fn read_gpt<B: SizedBlockDevice>(device: &B) -> Result<GPT, GPTReadError<B::BlockDeviceError>> {
    fn read<B: SizedBlockDevice>(
        device: &B,
        header_lba: LBA,
        primary_err: Option<InvalidHeader>,
    ) -> Result<(Header, Box<[PartitionEntry]>), GPTReadError<B::BlockDeviceError>> {
        let mut header_block = alloc_buffer_aligned(B::BLOCK_SIZE, align_of::<Header>())?;
        device.read_block(header_lba, &mut header_block)?;
        let header: Header = unsafe {
            let header_ptr = header_block.as_ptr();
            // Safety: any block_slice is aligned for Header and large enough
            // Also it is safe to create a header from any bit pattern.
            *header_ptr.cast()
        };

        if let Err(invalid_header) = header.verify(header_lba.get()) {
            return Err(GPTReadError::InvalidHeaders(
                primary_err.unwrap_or(invalid_header),
                invalid_header,
            ));
        }

        if header.number_of_partitions == 0 {
            if header.partition_entry_array_crc32 != 0 {
                let header_err =
                    InvalidHeader::InvalidEntryArrayCrc(header.partition_entry_array_crc32, 0);
                return Err(GPTReadError::InvalidHeaders(
                    primary_err.unwrap_or(header_err),
                    header_err,
                ));
            }

            return Ok((header, Box::try_new([])?));
        }

        let partition_array_block_count = counts_required_for!(
            B::BLOCK_SIZE as u64,
            (header.size_of_partition_entry * header.number_of_partitions) as u64
        );

        let array_start = LBA::new(header.partition_entry_lba).ok_or({
            let err = InvalidHeader::InvalidLBA(header.partition_entry_lba);
            GPTReadError::InvalidHeaders(primary_err.unwrap_or(err), err)
        })?;
        let mut array_data = alloc_buffer_aligned(
            partition_array_block_count as usize,
            align_of::<PartitionEntry>(),
        )?;
        let size = device.read_block_group(
            block_device::BlockGroup::with_count(
                array_start,
                NonZeroU64::new(partition_array_block_count)
                    .expect("Zero partitions handled above"),
            ),
            &mut array_data,
        )?;
        assert_eq!(size, partition_array_block_count as usize * B::BLOCK_SIZE);

        let mut partitions: Box<[MaybeUninit<PartitionEntry>]> =
            Box::try_new_uninit_slice(header.number_of_partitions as usize)?;

        let mut offset = 0;
        for to_initialize in partitions.iter_mut() {
            let entry = unsafe {
                assert!((offset as usize) < array_data.len());
                assert!(offset as usize + size_of::<PartitionEntry>() < array_data.len());

                let data_ptr = array_data.as_ptr().byte_offset(offset);
                // Safety:
                // data contains a bit pattern that is valid for a PartitionEntry.
                data_ptr.cast::<PartitionEntry>().read_unaligned()
            };
            to_initialize.write(entry);
            offset += header.size_of_partition_entry as isize;
        }

        let partitions = unsafe {
            // Safety: all entries were written above
            partitions.assume_init()
        };

        let partition_array_crc = {
            let array_len = header.number_of_partitions * header.size_of_partition_entry;
            let mut hasher = crc32fast::Hasher::new();
            hasher.update(&array_data[0..array_len as usize]);
            hasher.finalize()
        };

        if partition_array_crc != header.partition_entry_array_crc32 {
            let err = InvalidHeader::InvalidEntryArrayCrc(
                header.partition_entry_array_crc32,
                partition_array_crc,
            );
            return Err(GPTReadError::InvalidHeaders(err, err));
        }

        Ok((header, partitions))
    }

    let mut mbr_block = alloc_buffer_aligned(B::BLOCK_SIZE, align_of::<Header>())?;
    device.read_block(LBA::ZERO, &mut mbr_block)?;
    let mbr: &ProtectiveMBR = unsafe {
        let mbr_ptr = mbr_block.as_ptr();
        // Safety: any block_slice is aligned for ProtectedMBR and large enough
        // Also it is safe to create a protected mbr from any bit pattern.
        &*mbr_ptr.cast()
    };

    let warn_invalid_mbr = mbr.verify(device.size()).err();

    match read(device, LBA::ONE, None) {
        Ok((header, partitions)) => Ok(GPT {
            warn_invalid_mbr,
            warn_invalid_primary_header: None,
            partitions,
            header,
        }),
        Err(GPTReadError::InvalidHeaders(warn_invalid_primary_header, _)) => {
            let (backup_header, partitions) =
                read(device, device.last_lba(), Some(warn_invalid_primary_header))?;
            Ok(GPT {
                warn_invalid_mbr,
                warn_invalid_primary_header: Some(warn_invalid_primary_header),
                partitions,
                header: backup_header,
            })
        }
        Err(err) => Err(err),
    }
}
