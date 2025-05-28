use anyhow::{Context, Result};
use fuser::{FileAttr, FileType, Request};
use host_shared::sync::StdInterruptState;
#[allow(unused_imports)]
use libc::{EINVAL, EIO, ENOENT, ENOSYS};
use log::{debug, error, trace, warn};
use std::{
    path::Path,
    str::FromStr,
    sync::Arc,
    time::{Duration, UNIX_EPOCH},
};
use uuid::Uuid;
use wfs::{
    BLOCK_SIZE, blocks_required_for,
    fs::{FileSystem, FsError, FsRead, FsReadOnly, FsReadWrite, FsWrite, OverrideCheck},
    fs_structs::{self, FileId, FileNode, Perm, Timestamp},
    mem_structs,
};

use crate::{block_device::FileDevice, fuse::errno::fs_error_no};

pub mod errno;
pub mod fake;

macro_rules! handle_fs_err {
    ($expr:expr, $reply:expr) => {
        handle_fs_err!($expr, $reply, ())
    };
    ($expr:expr, $reply:expr, $err_res:expr) => {
        match $expr {
            Ok(res) => res,
            Err(err) => {
                log::error!("handle fs error: {err}");
                $reply.error(fs_error_no(err));
                return $err_res;
            }
        }
    };
}

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

impl<S: FsRead> WasabiFuse<S> {
    fn get_file_attr(&self, id: FileId) -> Result<Option<FileAttr>, FsError> {
        let file_node = self.fs().read_file_attr(id);

        file_node.map(|node| node.map(|node| self.map_node_to_attr(node)))
    }

    fn map_node_to_attr(&self, node: Arc<FileNode>) -> FileAttr {
        let mtime = UNIX_EPOCH + Duration::from_secs(node.modified_at.get());
        let crtime = UNIX_EPOCH + Duration::from_secs(node.created_at.get());
        let kind = match node.typ {
            fs_structs::FileType::File => FileType::RegularFile,
            fs_structs::FileType::Directory => FileType::Directory,
        };
        let perm: u16 = {
            let perm: [u8; 4] = unsafe { std::mem::transmute_copy(&node.permissions) };
            (perm[0] as u16) << 12 | (perm[1] as u16) << 8 | (perm[2] as u16) << 4 | perm[1] as u16
        };

        FileAttr {
            ino: node.id.get().try_into().unwrap(),
            size: node.size,
            blocks: blocks_required_for!(node.size),
            atime: mtime,
            mtime,
            ctime: mtime,
            crtime,
            kind,
            perm,
            nlink: 1,
            uid: self.uid,
            gid: self.gid,
            rdev: 0,
            blksize: BLOCK_SIZE as u32,
            flags: 0,
        }
    }
}

impl<S: FsRead + FsWrite> fuser::Filesystem for WasabiFuse<S> {
    fn init(
        &mut self,
        req: &Request<'_>,
        config: &mut fuser::KernelConfig,
    ) -> std::prelude::v1::Result<(), libc::c_int> {
        trace!("fuser::init");
        debug!("init request: config: {config:?}");
        debug!("gid: {}, uid: {}, pid: {}", req.gid(), req.uid(), req.pid());
        self.uid = req.uid();
        self.gid = req.gid();
        Ok(())
    }

    fn destroy(&mut self) {
        trace!("fuser::destroy");
        debug!("Shutdown");
        self.fs_mut().flush().unwrap();
    }

    fn readdir(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        _offset: i64,
        mut reply: fuser::ReplyDirectory,
    ) {
        trace!("fuser::readdir(ino: {ino})");
        let dir_id = match FileId::try_new(ino) {
            Some(id) => id,
            None => return reply.error(EINVAL),
        };

        let dir = handle_fs_err!(self.fs().read_directory(dir_id), reply);

        let mut last_full = false;
        for entry in dir.entries {
            assert!(!last_full);

            let Some(entry_meta) = handle_fs_err!(self.fs().read_file_attr(entry.id), reply) else {
                reply.error(EIO);
                return;
            };

            last_full = reply.add(
                entry.id.get(),
                0,
                map_file_type(entry_meta.typ),
                std::ffi::OsString::from_str(&entry.name).unwrap(),
            );
        }
        reply.ok();
    }

    fn lookup(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &std::ffi::OsStr,
        reply: fuser::ReplyEntry,
    ) {
        trace!("fuser::lookup(name: {name:?}, parent: {parent})");
        let Some(name) = name.to_str() else {
            reply.error(EINVAL);
            return;
        };

        let parent_id = match FileId::try_new(parent) {
            Some(id) => id,
            None => return reply.error(EINVAL),
        };

        let dir = handle_fs_err!(self.fs().read_directory(parent_id), reply);
        let Some(file_entry) = dir.entries.iter().find(|e| e.name.as_ref() == name) else {
            reply.error(ENOENT);
            return;
        };

        match self.get_file_attr(file_entry.id) {
            Ok(Some(file_attr)) => reply.entry(&self.ttl, &file_attr, 0),
            Ok(None) => reply.error(EIO),
            Err(e) => reply.error(fs_error_no(e)),
        }
    }

    fn getattr(&mut self, _req: &Request<'_>, ino: u64, _fh: Option<u64>, reply: fuser::ReplyAttr) {
        trace!("fuser::getattr(ino: {ino})");
        let file_id = match FileId::try_new(ino) {
            Some(id) => id,
            None => return reply.error(EINVAL),
        };
        let file_node = self.get_file_attr(file_id);

        match file_node {
            Ok(Some(file_attr)) => reply.attr(&self.ttl, &file_attr),
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

    fn mkdir(
        &mut self,
        _req: &Request<'_>,
        parent: u64,
        name: &std::ffi::OsStr,
        mode: u32,
        _umask: u32,
        reply: fuser::ReplyEntry,
    ) {
        trace!("fuser::mkdir(parent: {parent}, name: {name:?}, mode: {mode:o})");

        let parent_id = match FileId::try_new(parent) {
            Some(id) => id,
            None => return reply.error(EINVAL),
        };

        let uid = self.uid;
        let gid = self.gid;
        let fs = self.fs_mut();

        let _parent = handle_fs_err!(fs.read_directory(parent_id), reply);

        let new_dir = mem_structs::Directory::default();
        let new_dir_block = handle_fs_err!(new_dir.store(fs), reply).lba;

        let new_node = Arc::new(FileNode {
            id: fs.get_and_inc_file_id(),
            parent: Some(parent_id),
            typ: fs_structs::FileType::Directory,
            permissions: [Perm::empty(); 4],
            _unused: [0; 3],
            uid,
            gid,
            size: 0,
            created_at: Timestamp::zero(),
            modified_at: Timestamp::zero(),
            block_data: new_dir_block.into(),
        });

        handle_fs_err!(
            fs.mem_tree
                .insert(&mut fs.device, new_node.clone(), true)
                .map_err(FsError::from),
            reply
        );

        reply.entry(&self.ttl, &self.map_node_to_attr(new_node), 0);
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

fn map_file_type(typ: fs_structs::FileType) -> FileType {
    match typ {
        fs_structs::FileType::File => FileType::RegularFile,
        fs_structs::FileType::Directory => FileType::Directory,
    }
}
