use anyhow::Context;
use anyhow::Result;
use fuser::Request;
use log::debug;
use log::error;
use std::path::Path;
use std::time::Duration;
use uuid::Uuid;
use wfs::fs::FsWrite;
use wfs::fs::OverrideCheck;

use wfs::fs::{FileSystem, FsRead, FsReadOnly, FsReadWrite};

use crate::block_device::FileDevice;

pub mod fake;

pub struct WasabiFuse<S> {
    file_system: Option<FileSystem<FileDevice, S>>,
}

impl<S> WasabiFuse<S> {
    pub fn fs(&self) -> &FileSystem<FileDevice, S> {
        self.file_system.as_ref().expect("Only none during drop")
    }

    pub fn fs_mut(&mut self) -> &mut FileSystem<FileDevice, S> {
        self.file_system.as_mut().expect("Only none during drop")
    }
}

impl WasabiFuse<FsReadOnly> {
    pub fn open(image: &Path) -> Result<Self> {
        let device = FileDevice::open(image).context("failed to open image file")?;
        let fs = FileSystem::<_, FsReadOnly>::open(device).context("open filesystem readonly")?;

        Ok(WasabiFuse {
            file_system: Some(fs),
        })
    }
}

impl WasabiFuse<FsReadWrite> {
    pub fn open(image: &Path) -> Result<Self> {
        let device = FileDevice::open(image).context("failed to open image file")?;
        let fs = FileSystem::<_, FsReadWrite>::open(device).context("open filesystem readwrite")?;

        Ok(WasabiFuse {
            file_system: Some(fs),
        })
    }

    pub fn force_open(image: &Path) -> Result<Self> {
        let device = FileDevice::open(image).context("failed to open image file")?;
        let fs = FileSystem::<_, FsReadWrite>::force_open(device)
            .context("open filesystem readwrite")?;

        Ok(WasabiFuse {
            file_system: Some(fs),
        })
    }

    pub fn create(
        image: &Path,
        block_count: u64,
        override_check: OverrideCheck,
        uuid: Uuid,
        name: Option<Box<str>>,
    ) -> Result<Self> {
        let device =
            FileDevice::create(image, block_count).context("Failed to create file device")?;

        let fs = FileSystem::<_, FsReadWrite>::create(device, override_check, uuid, name)
            .expect("Failed to create filesystem");

        Ok(WasabiFuse {
            file_system: Some(fs),
        })
    }
}

impl<S: FsRead + FsWrite> fuser::Filesystem for WasabiFuse<S> {
    fn init(
        &mut self,
        req: &Request<'_>,
        config: &mut fuser::KernelConfig,
    ) -> std::prelude::v1::Result<(), libc::c_int> {
        debug!("init request: config: {config:?}");
        debug!("gid: {}, uid: {}, pid: {}", req.gid(), req.uid(), req.pid());
        Ok(())
    }

    fn destroy(&mut self) {
        debug!("Shutdown");
        self.fs_mut().flush().unwrap();
    }
}

impl<S> Drop for WasabiFuse<S> {
    fn drop(&mut self) {
        let mut fs = self.file_system.take().unwrap();
        for i in 0..10 {
            if i > 0 {
                error!("retry FS.close()");
            }
            match fs.close() {
                Ok(block_device) => {
                    block_device.close().unwrap();
                    return;
                }
                Err((fs_back, e)) => {
                    fs = fs_back;
                    error!("Close FS failed with: {:?}", e);
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}
