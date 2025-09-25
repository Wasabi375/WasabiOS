use std::{
    cmp::max,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    ops::Deref,
    path::Path,
    ptr::NonNull,
    sync::{Mutex, MutexGuard},
    usize,
};

use shared::iter::{IterExt, PositionInfo};
use thiserror::Error;
use wfs::{
    BLOCK_SIZE, Block, BlockAligned, BlockGroup, BlockSlice, LBA, blocks_required_for,
    interface::{BlockDevice, BlockDeviceOrMemError, WriteData},
};

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

    fn max_block_count(&self) -> Result<u64, Self::BlockDeviceError> {
        Ok(self.max_block_count)
    }

    fn read_block(&self, lba: LBA) -> Result<Box<BlockSlice>, Self::BlockDeviceError> {
        let mut block = Box::new(BlockAligned([0; BLOCK_SIZE]));

        let mut file = self.file.lock().unwrap();

        Self::read_contig_internal(&mut file, lba, &mut block.0)?;

        Ok(block)
    }

    fn read_blocks_contig(
        &self,
        start: LBA,
        block_count: u64,
    ) -> Result<Box<[u8]>, Self::BlockDeviceError> {
        let mut file = self.file.lock().unwrap();
        let mut buffer =
            unsafe { Box::new_zeroed_slice(block_count as usize * BLOCK_SIZE).assume_init() };

        Self::read_contig_internal(&mut file, start, &mut buffer)?;

        Ok(buffer)
    }

    fn read_blocks<I>(
        &self,
        blocks: I,
    ) -> Result<Box<[u8]>, BlockDeviceOrMemError<Self::BlockDeviceError>>
    where
        I: Iterator<Item = BlockGroup> + Clone,
    {
        let total_bytes: u64 = blocks.clone().map(|group| group.bytes()).sum();
        let total_bytes = total_bytes as usize;

        let buffer = Box::<[u8]>::try_new_zeroed_slice(total_bytes)
            .map_err(|_| BlockDeviceOrMemError::Allocation)?;

        let mut buffer = unsafe {
            // Safety: u8 has no invalid states
            buffer.assume_init()
        };

        let mut file = self.file.lock().unwrap();

        let mut cursor = 0;
        for group in blocks {
            // cursor 1 past end for rust range and easy copy as the next start
            let end_cursor = cursor + group.bytes() as usize;
            Self::read_contig_internal(&mut file, group.start, &mut buffer[cursor..end_cursor])?;

            cursor = end_cursor;
        }

        assert_eq!(cursor, total_bytes);

        Ok(buffer)
    }

    fn write_block(
        &mut self,
        lba: LBA,
        data: NonNull<BlockSlice>,
    ) -> Result<(), Self::BlockDeviceError> {
        let mut file = self.file.lock().unwrap();

        file.seek(SeekFrom::Start(lba.get() * BLOCK_SIZE as u64))?;

        let data = unsafe { data.as_ref() };

        if file.write(&data.0)? != BLOCK_SIZE {
            return Err(Self::BlockDeviceError::other(
                FileDevicError::IncompleteWrite,
            ));
        }

        Ok(())
    }

    fn write_blocks_contig(
        &mut self,
        start: LBA,
        data: NonNull<[u8]>,
    ) -> Result<(), Self::BlockDeviceError> {
        assert_eq!(data.len() % BLOCK_SIZE, 0);
        let mut file = self.file.lock().unwrap();

        file.seek(SeekFrom::Start(start.get() * BLOCK_SIZE as u64))?;

        if file.write(unsafe { data.as_ref() })? != data.len() {
            return Err(Self::BlockDeviceError::other(
                FileDevicError::IncompleteWrite,
            ));
        }

        Ok(())
    }

    fn write_blocks<I>(
        &mut self,
        blocks: I,
        data: WriteData,
    ) -> Result<(), BlockDeviceOrMemError<Self::BlockDeviceError>>
    where
        I: Iterator<Item = BlockGroup> + Clone,
    {
        assert!(data.is_valid());

        assert_eq!(
            blocks_required_for!(data.total_len()),
            blocks.clone().map(|b| b.count()).sum(),
            "data.total_len() should fit into exactly the number of blocks specified"
        );

        let mut unwritten = data.data;

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
                let mut block_buffer = Box::try_new(Block::new([0; BLOCK_SIZE]))
                    .map_err(|_| BlockDeviceOrMemError::Allocation)?;

                let data_start = data.old_block_start.len();
                let data_past_end = data_start + data.data.len();

                block_buffer.data[..data_start].copy_from_slice(data.old_block_start);
                block_buffer.data[data_start..data_past_end].copy_from_slice(data.data);
                block_buffer.data[data_past_end..].copy_from_slice(data.old_block_end);

                self.write_block(block_group.start, block_buffer.block_data())?;
                break;
            }

            if first && data.old_block_start.len() > 0 {
                let mut first_block_buffer = Box::try_new(Block::new([0; BLOCK_SIZE]))
                    .map_err(|_| BlockDeviceOrMemError::Allocation)?;

                first_block_buffer.data[..data.old_block_start.len()]
                    .copy_from_slice(data.old_block_start);

                let first_block_data_len = BLOCK_SIZE - data.old_block_start.len();
                let to_write = unwritten
                    .split_off(..first_block_data_len)
                    .expect("total_length should be enough for first block");
                first_block_buffer.data[data.old_block_start.len()..].copy_from_slice(to_write);

                self.write_block(block_group.start, first_block_buffer.block_data())?;

                block_group = block_group.subgroup(1);
            }

            if last && data.old_block_end.len() > 0 {
                let mut last_block_buffer = Box::try_new(Block::new([0; BLOCK_SIZE]))
                    .map_err(|_| BlockDeviceOrMemError::Allocation)?;

                let single_block = block_group.count() == 1;

                let last_block_lba = block_group.end();
                let last_block_data_offset = if !single_block {
                    block_group = block_group.remove_end(1);
                    block_group.bytes() as usize
                } else {
                    0
                };

                let last_block_to_write_slice = &unwritten
                    .split_off(last_block_data_offset..)
                    .expect("There should be enough data for the last block");

                last_block_buffer.data[..last_block_to_write_slice.len()]
                    .copy_from_slice(last_block_to_write_slice);

                self.write_block(last_block_lba, last_block_buffer.block_data())?;

                if single_block {
                    break;
                }
            }

            let to_write: &[u8] = unwritten
                .split_off(..block_group.bytes() as usize)
                .expect("There should be enough data left, because we checked at function start and special handle first and last blocks");

            assert_eq!(to_write.len() % BLOCK_SIZE, 0);

            self.write_blocks_contig(block_group.start, to_write.into())?;
        }

        assert!(unwritten.is_empty());

        Ok(())
    }

    fn read_block_atomic(&self, lba: LBA) -> Result<Box<BlockSlice>, Self::BlockDeviceError> {
        self.read_block(lba)
    }

    fn write_block_atomic(
        &mut self,
        lba: LBA,
        data: NonNull<BlockSlice>,
    ) -> Result<(), Self::BlockDeviceError> {
        self.write_block(lba, data)
    }

    fn compare_exchange_block(
        &mut self,
        lba: LBA,
        current: NonNull<BlockSlice>,
        new: NonNull<BlockSlice>,
    ) -> Result<Result<(), Box<BlockSlice>>, Self::BlockDeviceError> {
        let current = unsafe { current.as_ref() };
        let new = unsafe { new.as_ref() };

        let mut file = self.file.lock().unwrap();
        file.seek(SeekFrom::Start(lba.get() * BLOCK_SIZE as u64))?;

        let mut block = [0; BLOCK_SIZE]; // TODO move buffer to heap?
        if file.read(block.as_mut())? != BLOCK_SIZE {
            return Err(Self::BlockDeviceError::other(FileDevicError::UnexpectedEOF));
        }

        if &block != &current.0 {
            return Ok(Err(Box::new(BlockAligned(block))));
        }

        file.seek(SeekFrom::Start(lba.get() * BLOCK_SIZE as u64))?;
        if file.write(new.deref())? != BLOCK_SIZE {
            return Err(Self::BlockDeviceError::other(
                FileDevicError::IncompleteWrite,
            ));
        }

        Ok(Ok(()))
    }
}
