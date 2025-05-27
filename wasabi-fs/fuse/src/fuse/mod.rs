use anyhow::Context;
use anyhow::Result;
use fuser::FileAttr;
use fuser::FileType;
use fuser::Request;
use host_shared::sync::StdInterruptState;
use libc::EINVAL;
use libc::ENOENT;
use log::debug;
use log::error;
use log::warn;
use shared::todo_warn;
use std::path::Path;
use std::time::Duration;
use std::time::UNIX_EPOCH;
use uuid::Uuid;
use wfs::BLOCK_SIZE;
use wfs::blocks_required_for;
use wfs::fs::FsWrite;
use wfs::fs::OverrideCheck;
use wfs::fs_structs;
use wfs::fs_structs::FileId;

use wfs::fs::{FileSystem, FsRead, FsReadOnly, FsReadWrite};

use crate::block_device::FileDevice;
use crate::fuse::errno::fs_error_no;

pub mod errno;
pub mod fake;

pub struct WasabiFuse<S> {
    file_system: Option<FileSystem<FileDevice, S, StdInterruptState>>,
    /// Time to live
    ///
    /// the time the kernel will cache any data provided by this driver
    pub ttl: Duration,

    uid: u32,
    gid: u32,
}

impl<S> WasabiFuse<S> {
    pub fn fs(&self) -> &FileSystem<FileDevice, S, StdInterruptState> {
        self.file_system.as_ref().expect("Only none during drop")
    }

    pub fn fs_mut(&mut self) -> &mut FileSystem<FileDevice, S, StdInterruptState> {
        self.file_system.as_mut().expect("Only none during drop")
    }
}

impl WasabiFuse<FsReadOnly> {
    pub fn open(image: &Path) -> Result<Self> {
        let device = FileDevice::open(image).context("failed to open image file")?;
        let fs =
            FileSystem::<_, FsReadOnly, _>::open(device).context("open filesystem readonly")?;

        Ok(WasabiFuse {
            file_system: Some(fs),
            ttl: Duration::from_secs(1),
            uid: 0,
            gid: 0,
        })
    }
}

impl WasabiFuse<FsReadWrite> {
    pub fn open(image: &Path) -> Result<Self> {
        let device = FileDevice::open(image).context("failed to open image file")?;
        let fs =
            FileSystem::<_, FsReadWrite, _>::open(device).context("open filesystem readwrite")?;

        Ok(WasabiFuse {
            file_system: Some(fs),
            ttl: Duration::from_secs(1),
            uid: 0,
            gid: 0,
        })
    }

    pub fn force_open(image: &Path) -> Result<Self> {
        let device = FileDevice::open(image).context("failed to open image file")?;
        let fs = FileSystem::<_, FsReadWrite, _>::force_open(device)
            .context("open filesystem readwrite")?;

        Ok(WasabiFuse {
            file_system: Some(fs),
            ttl: Duration::from_secs(1),
            uid: 0,
            gid: 0,
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

        let fs = FileSystem::<_, FsReadWrite, _>::create(device, override_check, uuid, name)
            .expect("Failed to create filesystem");

        Ok(WasabiFuse {
            file_system: Some(fs),
            ttl: Duration::from_secs(1),
            uid: 0,
            gid: 0,
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
        self.uid = req.uid();
        self.gid = req.gid();
        Ok(())
    }

    fn destroy(&mut self) {
        debug!("Shutdown");
        self.fs_mut().flush().unwrap();
    }

    #[allow(unused)]
    fn getattr(
        &mut self,
        _req: &Request<'_>,
        mut ino: u64,
        _fh: Option<u64>,
        reply: fuser::ReplyAttr,
    ) {
        let file_id = match FileId::try_new(ino) {
            Some(id) => id,
            None => return reply.error(EINVAL),
        };
        let file_node = self.fs().read_file_attr(file_id);

        match file_node {
            Ok(Some(file_node)) => {
                let mtime = UNIX_EPOCH + Duration::from_secs(file_node.modified_at.get());
                let crtime = UNIX_EPOCH + Duration::from_secs(file_node.created_at.get());
                let (kind, nlink) = match file_node.typ {
                    fs_structs::FileType::File => (FileType::RegularFile, 1),
                    fs_structs::FileType::Directory => (FileType::Directory, 2),
                };
                let perm: u16 = {
                    let perm: [u8; 4] = unsafe { std::mem::transmute_copy(&file_node.permissions) };
                    (perm[0] as u16) << 12
                        | (perm[1] as u16) << 8
                        | (perm[2] as u16) << 4
                        | perm[1] as u16
                };
                reply.attr(
                    &self.ttl,
                    &dbg!(FileAttr {
                        ino: file_node.id.get().try_into().unwrap(),
                        size: file_node.size,
                        blocks: blocks_required_for!(file_node.size),
                        atime: mtime,
                        mtime,
                        ctime: mtime,
                        crtime,
                        kind,
                        // TODO ensure that ordering matches once I decide how
                        // I want to handle this in my fs
                        perm,
                        nlink,
                        uid: self.uid,
                        gid: self.gid,
                        rdev: 0,
                        blksize: BLOCK_SIZE as u32,
                        flags: 0,
                    }),
                )
            }
            Ok(None) => {
                warn!("getattr: inode {ino} not found!");
                reply.error(ENOENT)
            }
            Err(e) => {
                error!("Failed to get inode({ino}): {e}");
                reply.error(fs_error_no(e));
            }
        }
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
