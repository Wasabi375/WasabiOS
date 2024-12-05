use std::{
    cmp::max,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    ptr::NonNull,
    sync::Mutex,
};

use thiserror::Error;
use wfs::{interface::BlockDevice, BlockSlice, BLOCK_SIZE, LBA};

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
        let mut file = self.file.lock().unwrap();

        file.seek(SeekFrom::Start(lba.get() * BLOCK_SIZE as u64))?;

        let mut block = Box::new([0; BLOCK_SIZE]);
        if file.read(block.as_mut())? != BLOCK_SIZE {
            return Err(Self::BlockDeviceError::other(FileDevicError::UnexpectedEOF));
        }

        Ok(block)
    }

    fn read_blocks(
        &self,
        start: LBA,
        block_count: u64,
    ) -> Result<Box<[u8]>, Self::BlockDeviceError> {
        let mut file = self.file.lock().unwrap();

        file.seek(SeekFrom::Start(start.get() * BLOCK_SIZE as u64))?;

        let mut buffer =
            unsafe { Box::new_zeroed_slice(block_count as usize * BLOCK_SIZE).assume_init() };
        if file.read(buffer.as_mut())? != buffer.len() {
            return Err(Self::BlockDeviceError::other(FileDevicError::UnexpectedEOF));
        }

        Ok(buffer)
    }

    fn write_block(
        &mut self,
        lba: LBA,
        data: NonNull<BlockSlice>,
    ) -> Result<(), Self::BlockDeviceError> {
        let mut file = self.file.lock().unwrap();

        file.seek(SeekFrom::Start(lba.get() * BLOCK_SIZE as u64))?;

        if file.write(unsafe { data.as_ref() })? != BLOCK_SIZE {
            return Err(Self::BlockDeviceError::other(
                FileDevicError::IncompleteWrite,
            ));
        }

        Ok(())
    }

    fn write_blocks(
        &mut self,
        start: LBA,
        data: NonNull<[u8]>,
    ) -> Result<(), Self::BlockDeviceError> {
        let mut file = self.file.lock().unwrap();

        file.seek(SeekFrom::Start(start.get() * BLOCK_SIZE as u64))?;

        if file.write(unsafe { data.as_ref() })? != data.len() {
            return Err(Self::BlockDeviceError::other(
                FileDevicError::IncompleteWrite,
            ));
        }

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

        let mut block = [0; BLOCK_SIZE];
        if file.read(block.as_mut())? != BLOCK_SIZE {
            return Err(Self::BlockDeviceError::other(FileDevicError::UnexpectedEOF));
        }

        if &block != current {
            return Ok(Err(Box::new(block)));
        }

        file.seek(SeekFrom::Start(lba.get() * BLOCK_SIZE as u64))?;
        if file.write(new)? != BLOCK_SIZE {
            return Err(Self::BlockDeviceError::other(
                FileDevicError::IncompleteWrite,
            ));
        }

        Ok(Ok(()))
    }
}
