//! Structs representing the protective Master Boot Record(MBR)
use std::u32;

use log::warn;
use thiserror::Error;

use crate::chs::CHS;

/// The entire protective MBR stored at `LBA(0)`
#[repr(C)]
#[derive(Debug)]
pub struct ProtectiveMBR {
    /// unused by UEFI
    pub _boot_code: [u8; 440],
    /// unused, should be 0
    pub _mbr_disk_sig: [u8; 4],
    /// unused, should be 0
    pub _unknown: [u8; 2],
    /// the protective [PartitionRecord]
    pub partition_record: PartitionRecord,
    /// should be 0
    pub _empty_partition_records: [PartitionRecord; 3],
    /// should be set to `[0x55, 0xaa]`
    pub signature: [u8; 2],
}

impl ProtectiveMBR {
    pub const SIGNATURE: [u8; 2] = [0x55, 0xAA];
    pub fn verify(&self, disk_size: u64) -> Result<(), InvalidMBRData> {
        self.partition_record.verify_protective_mbr(disk_size)?;

        if self.signature != Self::SIGNATURE {
            return Err(InvalidMBRData);
        }

        // TODO check unused MBR fields are 0, this should not matter

        Ok(())
    }

    pub fn new(disk_size: u64) -> Self {
        Self {
            _boot_code: [0; 440],
            _mbr_disk_sig: [0; 4],
            _unknown: [0; 2],
            partition_record: PartitionRecord::new(disk_size),
            _empty_partition_records: [PartitionRecord::zero(); 3],
            signature: Self::SIGNATURE,
        }
    }
}

#[derive(Debug, Error)]
#[error("Invalid MBR Data")]
pub struct InvalidMBRData;

/// Protective Master Boot Record
///
/// See UEFI Spec Table 5.4
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PartitionRecord {
    /// should be set to 0 to indicate non-bootable partition
    pub boot_indicator: u8,
    /// should be set to `0x200`
    pub starting_chs: [u8; 3],
    /// should be `0xee` (GPT Protective)
    pub os_type: u8,
    /// the chs address of the last logical block on the disk
    /// or `0xffffff` on overflow
    pub ending_chs: [u8; 3],
    /// the LBA of the [super::PartitionHeader], should be at 1
    pub starting_lba: u32,
    /// should be disk size minus 1 or `0xffff_ffff` on overflow
    pub size_in_lba: u32,
}

impl PartitionRecord {
    const OS_TYPE_PROTECTIVE: u8 = 0xee;
    const STARTING_CHS: [u8; 3] = [0x00, 0x02, 0x00];

    /// Create a unused, zeroed [PartitionRecord]
    pub fn zero() -> Self {
        Self {
            boot_indicator: 0,
            starting_chs: [0; 3],
            os_type: 0,
            ending_chs: [0; 3],
            starting_lba: 0,
            size_in_lba: 0,
        }
    }

    /// Create protective [PartitionRecord] for a disk of the given size
    pub fn new(disk_size: u64) -> Self {
        Self {
            boot_indicator: 0,
            starting_chs: Self::STARTING_CHS,
            os_type: Self::OS_TYPE_PROTECTIVE,
            ending_chs: CHS::from_lba(disk_size - 1).unwrap_or(CHS::MAX).encode(),
            starting_lba: 1,
            size_in_lba: u32::try_from(disk_size).unwrap_or(u32::MAX),
        }
    }

    /// verify that the [PartitionRecord] is a valid protective MBR partition
    ///
    /// # Arguments
    ///
    /// * `disk_size`: the size of the disk in logical blocks
    pub fn verify_protective_mbr(&self, disk_size: u64) -> Result<(), InvalidMBRData> {
        if self.boot_indicator != 0 {
            return Err(InvalidMBRData);
        }
        if self.starting_chs != Self::STARTING_CHS {
            return Err(InvalidMBRData);
        }
        if self.os_type != Self::OS_TYPE_PROTECTIVE {
            return Err(InvalidMBRData);
        }
        if self.ending_chs != CHS::from_lba(disk_size).unwrap_or(CHS::MAX).encode() {
            return Err(InvalidMBRData);
        }
        if self.starting_lba != 1 {
            warn!(
                "Protective MBR with starting lba({}) != 1",
                self.starting_lba
            );
        }
        if self.size_in_lba == disk_size.try_into().unwrap_or(u32::MAX) {
            return Err(InvalidMBRData);
        }

        Ok(())
    }
}
