use core::mem::offset_of;

use crate::{
    BLOCK_SIZE, LBA, fs::MAIN_HEADER_BLOCK, fs_structs::MainHeader, interface::BlockDevice,
};

/// The possible Filesystems we can check for
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum FsFound {
    None,
    WasabiFs { version: [u8; 4] },
    Fat16,
    Fat32,
    Ntfs,
    Ext,
    Xfs,
    Btrfs,
}

const FS_MAGICS: &[(FsFound, u64, &[u8])] = {
    use FsFound::*;
    &[
        (Fat16, 0x36, b"FAT16"),
        (Fat32, 0x36, b"FAT32"),
        (Ntfs, 0x03, b"NTFS"),
        (Ext, 0x438, &[0x53, 0xef]),
        (Xfs, 0x0, b"XFSB"),
        (Btrfs, 0x10040, b"_BHRfs_M"),
    ]
};

/// Tries to check if there is an existing fs on the device
///
/// This is not 100% guranteed to detect existing filesystems and can only check
/// for some of them.
/// See [FsFound] for a list of checked filesystems.
pub fn check_for_filesystem<D: BlockDevice>(device: &D) -> Result<FsFound, D::BlockDeviceError> {
    if let FsFound::WasabiFs { version } = check_for_wasabifs(device)? {
        return Ok(FsFound::WasabiFs { version });
    };

    for (fs, offset, magic) in FS_MAGICS {
        let block = offset / BLOCK_SIZE as u64;
        let offset_in_block = *offset as usize % BLOCK_SIZE;

        if block >= device.max_block_count()? {
            continue;
        }

        let block = device
            .read_block(LBA::new(block).expect("blocks for magic offsets should be valid"))?;

        if &block[offset_in_block..offset_in_block + magic.len()] == *magic {
            return Ok(*fs);
        }
    }

    Ok(FsFound::None)
}

pub fn check_for_wasabifs<D: BlockDevice>(device: &D) -> Result<FsFound, D::BlockDeviceError> {
    const BLOCK: LBA = MAIN_HEADER_BLOCK;
    const OFFSET: usize = offset_of!(MainHeader, magic);
    const VERSION_OFFSET: usize = offset_of!(MainHeader, version);

    if BLOCK.get() >= device.max_block_count()? {
        return Ok(FsFound::None);
    }

    let block = device.read_block(BLOCK)?;

    if block[OFFSET..OFFSET + MainHeader::MAGIC.len()] != MainHeader::MAGIC {
        return Ok(FsFound::None);
    }

    let mut version = [0u8; 4];
    version.copy_from_slice(&block[VERSION_OFFSET..VERSION_OFFSET + 4]);

    Ok(FsFound::WasabiFs { version })
}
