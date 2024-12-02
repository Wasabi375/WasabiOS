use std::{
    cmp::max,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    sync::Mutex,
};

use thiserror::Error;
use wfs::{interface::BlockDevice, BlockSlice, BLOCK_SIZE, LBA};

pub struct FileDevice {
    max_block_count: u64,
    file: Mutex<File>,
}

impl FileDevice {
    pub fn open(path: &Path, block_count: u64) -> FileDevice {
        let mut file = File::options().write(true).create(true).open(path).unwrap();

        let old_size = file.seek(SeekFrom::End(0)).unwrap();
        let new_size = max(old_size, block_count * BLOCK_SIZE as u64);
        file.seek(SeekFrom::Start(0)).unwrap();

        file.set_len(new_size).unwrap();

        FileDevice {
            max_block_count: block_count,
            file: Mutex::new(file),
        }
    }

    pub fn close(self) {
        self.file.lock().unwrap().flush().unwrap();
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
        data: std::ptr::NonNull<BlockSlice>,
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
        data: std::ptr::NonNull<[u8]>,
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
        data: std::ptr::NonNull<BlockSlice>,
    ) -> Result<(), Self::BlockDeviceError> {
        self.write_block(lba, data)
    }

    fn compare_exchange_block(
        &mut self,
        lba: LBA,
        current: &BlockSlice,
        new: &BlockSlice,
    ) -> Result<Result<(), Box<BlockSlice>>, Self::BlockDeviceError> {
        let mut file = self.file.lock().unwrap();
        file.seek(SeekFrom::Start(lba.get() * BLOCK_SIZE as u64))?;

        let mut block = [0; BLOCK_SIZE];
        if file.read(block.as_mut())? != BLOCK_SIZE {
            return Err(Self::BlockDeviceError::other(FileDevicError::UnexpectedEOF));
        }

        if &block != current {
            return Ok(Err(Box::new(block)));
        }

        if file.write(new.as_ref())? != BLOCK_SIZE {
            return Err(Self::BlockDeviceError::other(
                FileDevicError::IncompleteWrite,
            ));
        }

        Ok(Ok(()))
    }
}
