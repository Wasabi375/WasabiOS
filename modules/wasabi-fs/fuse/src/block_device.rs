use std::{
    cmp::max,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    sync::{Mutex, MutexGuard},
};

use block_device::{
    BlockDevice, BlockGroup, CompareExchangeError, LBA, ReadBlockDeviceError,
    WriteBlockDeviceError, WriteData,
};
use log::warn;
use shared::{
    alloc_ext::alloc_buffer,
    iter::{IterExt, PositionInfo},
};
use thiserror::Error;
use wfs::{BLOCK_SIZE, blocks_required_for};

pub struct FileDevice {
    max_block_count: u64,
    file: Mutex<File>,
}

impl FileDevice {
    pub fn create(path: &Path, block_count: u64) -> Result<FileDevice, std::io::Error> {
        let mut file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .open(path)?;

        let old_size = file.seek(SeekFrom::End(0))?;
        let new_size = max(old_size, block_count * BLOCK_SIZE as u64);
        file.seek(SeekFrom::Start(0))?;

        file.set_len(new_size)?;

        Ok(FileDevice {
            max_block_count: block_count,
            file: Mutex::new(file),
        })
    }

    pub fn open(path: &Path) -> Result<FileDevice, std::io::Error> {
        let mut file = File::options().read(true).write(true).open(path)?;

        let size = file.seek(SeekFrom::End(0))?;
        file.seek(SeekFrom::Start(0))?;

        let block_count = size / BLOCK_SIZE as u64;

        Ok(FileDevice {
            max_block_count: block_count,
            file: Mutex::new(file),
        })
    }

    pub fn close(self) -> Result<(), std::io::Error> {
        self.file.lock().expect("We never shatter the lock").flush()
    }

    fn read_contig_internal(
        file: &mut MutexGuard<'_, File>,
        start: LBA,
        buffer: &mut [u8],
    ) -> Result<(), std::io::Error> {
        file.seek(SeekFrom::Start(start.get() * BLOCK_SIZE as u64))?;

        if file.read(buffer)? != buffer.len() {
            return Err(std::io::Error::other(FileDevicError::UnexpectedEOF));
        }

        Ok(())
    }
}

#[derive(Error, Debug)]
pub enum FileDevicError {
    #[error("unexpected EOF")]
    UnexpectedEOF,
    #[error("write was incomplete")]
    IncompleteWrite,
}

impl BlockDevice for FileDevice {
    type BlockDeviceError = std::io::Error;

    const BLOCK_SIZE: usize = 4096;

    fn size(&self) -> u64 {
        self.max_block_count
    }

    fn read_block(
        &self,
        lba: LBA,
        buffer: &mut [u8],
    ) -> Result<(), ReadBlockDeviceError<Self::BlockDeviceError>> {
        assert_eq!(buffer.len(), BLOCK_SIZE);

        let mut file = self.file.lock().unwrap();

        Self::read_contig_internal(&mut file, lba, buffer)?;

        Ok(())
    }

    fn read_blocks_contig(
        &self,
        start: LBA,
        block_count: u64,
        buffer: &mut [u8],
    ) -> Result<(), ReadBlockDeviceError<std::io::Error>> {
        assert_eq!(buffer.len(), BLOCK_SIZE * block_count as usize);

        let mut file = self.file.lock().unwrap();

        Self::read_contig_internal(&mut file, start, buffer)?;

        Ok(())
    }

    fn read_blocks<I>(
        &self,
        blocks: I,
        buffer: &mut [u8],
    ) -> Result<(), ReadBlockDeviceError<Self::BlockDeviceError>>
    where
        I: Iterator<Item = BlockGroup> + Clone,
    {
        let total_bytes_to_read: usize = blocks
            .clone()
            .map(|group| group.bytes(BLOCK_SIZE) as usize)
            .sum();
        assert!(total_bytes_to_read < buffer.len());
        assert_eq!(buffer.len() % BLOCK_SIZE, 0);

        let mut file = self.file.lock().unwrap();

        let mut cursor = 0;
        for group in blocks {
            // cursor 1 past end for rust range and easy copy as the next start
            let end_cursor = cursor + group.bytes(BLOCK_SIZE) as usize;
            Self::read_contig_internal(&mut file, group.start, &mut buffer[cursor..end_cursor])?;

            cursor = end_cursor;
        }

        assert_eq!(cursor, total_bytes_to_read);

        Ok(())
    }

    fn write_block(
        &mut self,
        lba: LBA,
        data: &[u8],
    ) -> Result<(), WriteBlockDeviceError<Self::BlockDeviceError>> {
        if data.len() != Self::BLOCK_SIZE {
            return Err(WriteBlockDeviceError::NotEnoughData {
                expected: Self::BLOCK_SIZE,
                size: data.len(),
            });
        }

        let mut file = self.file.lock().unwrap();

        file.seek(SeekFrom::Start(lba.get() * BLOCK_SIZE as u64))?;

        if file.write(data)? != BLOCK_SIZE {
            return Err(WriteBlockDeviceError::BlockDevice(
                Self::BlockDeviceError::other(FileDevicError::IncompleteWrite),
            ));
        }

        Ok(())
    }

    fn write_blocks_contig(
        &mut self,
        start: LBA,
        data: &[u8],
    ) -> Result<(), WriteBlockDeviceError<Self::BlockDeviceError>> {
        assert_eq!(data.len() % BLOCK_SIZE, 0);

        let mut file = self.file.lock().unwrap();

        file.seek(SeekFrom::Start(start.get() * BLOCK_SIZE as u64))?;

        if file.write(data)? != data.len() {
            return Err(WriteBlockDeviceError::BlockDevice(
                Self::BlockDeviceError::other(FileDevicError::IncompleteWrite),
            ));
        }

        Ok(())
    }

    fn write_blocks<I>(
        &mut self,
        blocks: I,
        data: WriteData,
    ) -> Result<(), WriteBlockDeviceError<Self::BlockDeviceError>>
    where
        I: Iterator<Item = BlockGroup> + Clone,
    {
        assert!(data.is_valid_for(BLOCK_SIZE));

        assert_eq!(
            blocks_required_for!(data.total_len()),
            blocks.clone().map(|b| b.count()).sum(),
            "data.total_len() should fit into exactly the number of blocks specified"
        );

        let mut unwritten = data.data;

        let mut block_buffer =
            alloc_buffer(BLOCK_SIZE).map_err(|_| WriteBlockDeviceError::Allocation)?;

        for PositionInfo {
            index: _,
            first,
            last,
            item: mut block_group,
        } in blocks.with_positions()
        {
            if first
                && last
                && block_group.count() == 1
                && data.old_block_start.len() > 0
                && data.old_block_end.len() > 0
            {
                // There is old data at the start and end. If there is just old data at start or
                // end, the normal first and last checks handle that case

                let data_start = data.old_block_start.len();
                let data_past_end = data_start + data.data.len();

                block_buffer[..data_start].copy_from_slice(data.old_block_start);
                block_buffer[data_start..data_past_end].copy_from_slice(data.data);
                block_buffer[data_past_end..].copy_from_slice(data.old_block_end);

                self.write_block(block_group.start, block_buffer.as_mut())?;
                break;
            }

            if first && data.old_block_start.len() > 0 {
                block_buffer[..data.old_block_start.len()].copy_from_slice(data.old_block_start);

                let first_block_data_len = BLOCK_SIZE - data.old_block_start.len();
                let to_write = unwritten
                    .split_off(..first_block_data_len)
                    .expect("total_length should be enough for first block");
                block_buffer[data.old_block_start.len()..].copy_from_slice(to_write);

                self.write_block(block_group.start, block_buffer.as_mut())?;

                block_group = block_group.subgroup(1);
            }

            if last && data.old_block_end.len() > 0 {
                let single_block = block_group.count() == 1;

                let last_block_lba = block_group.end();
                let last_block_data_offset = if !single_block {
                    block_group = block_group.remove_end(1);
                    block_group.bytes(BLOCK_SIZE) as usize
                } else {
                    0
                };

                let last_block_to_write_slice = &unwritten
                    .split_off(last_block_data_offset..)
                    .expect("There should be enough data for the last block");

                block_buffer[..last_block_to_write_slice.len()]
                    .copy_from_slice(last_block_to_write_slice);

                self.write_block(last_block_lba, block_buffer.as_mut())?;

                if single_block {
                    break;
                }
            }

            let to_write: &[u8] = unwritten
                .split_off(..block_group.bytes(BLOCK_SIZE) as usize)
                .expect("There should be enough data left, because we checked at function start and special handle first and last blocks");

            assert_eq!(to_write.len() % BLOCK_SIZE, 0);

            self.write_blocks_contig(block_group.start, to_write.into())?;
        }

        assert!(unwritten.is_empty());

        Ok(())
    }

    fn read_block_atomic(
        &self,
        lba: LBA,
        buffer: &mut [u8],
    ) -> Result<(), ReadBlockDeviceError<Self::BlockDeviceError>> {
        warn!("unimplemented atomic operation(read_block). Falling back to non-atomic version");
        self.read_block(lba, buffer)
    }

    fn write_block_atomic(
        &mut self,
        lba: LBA,
        data: &[u8],
    ) -> Result<(), WriteBlockDeviceError<Self::BlockDeviceError>> {
        warn!("unimplemented atomic operation(write_block). Falling back to non-atomic version");
        self.write_block(lba, data)
    }

    fn compare_exchange_block(
        &mut self,
        lba: LBA,
        current: &mut [u8],
        new: &[u8],
    ) -> Result<(), CompareExchangeError<Self::BlockDeviceError>> {
        assert_eq!(current.len(), Self::BLOCK_SIZE);
        assert_eq!(new.len(), Self::BLOCK_SIZE);

        warn!("unimplemented atomic operation(compare_exchange). Implementation is not atomic");

        let mut file = self.file.lock().unwrap();
        file.seek(SeekFrom::Start(lba.get() * BLOCK_SIZE as u64))?;

        let mut block = [0; BLOCK_SIZE];
        if file.read(block.as_mut())? != BLOCK_SIZE {
            return Err(Self::BlockDeviceError::other(FileDevicError::UnexpectedEOF).into());
        }

        if block != current {
            current.copy_from_slice(&block);
            return Err(CompareExchangeError::OutdatedData);
        }

        file.seek(SeekFrom::Start(lba.get() * BLOCK_SIZE as u64))?;
        if file.write(new)? != BLOCK_SIZE {
            return Err(Self::BlockDeviceError::other(FileDevicError::IncompleteWrite).into());
        }

        Ok(())
    }
}
