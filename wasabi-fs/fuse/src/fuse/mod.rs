use std::{
    ffi::OsStr,
    time::{Duration, UNIX_EPOCH},
};

use fuser::{
    FileAttr, FileType, KernelConfig, ReplyAttr, ReplyData, ReplyDirectory, ReplyEntry, Request,
};
use libc::{c_int, ENOENT, ENOTDIR};
use log::{debug, warn};

pub struct WasabiFuseTest;

const ROOT: FileAttr = FileAttr {
    ino: 1,
    size: 0,
    blocks: 0,
    atime: UNIX_EPOCH,
    mtime: UNIX_EPOCH,
    ctime: UNIX_EPOCH,
    crtime: UNIX_EPOCH,
    kind: FileType::Directory,
    perm: 0o755,
    nlink: 2,
    uid: 501,
    gid: 20,
    rdev: 0,
    blksize: 0,
    flags: 0,
};

const HELLO_TXT_CONTENT: &str = "Hello World!\n";
const HELLO: FileAttr = FileAttr {
    ino: 2,
    size: 13,
    blocks: 1,
    atime: UNIX_EPOCH,
    mtime: UNIX_EPOCH,
    ctime: UNIX_EPOCH,
    crtime: UNIX_EPOCH,
    kind: FileType::RegularFile,
    perm: 0o755,
    nlink: 1,
    uid: 501,
    gid: 20,
    rdev: 0,
    blksize: 512,
    flags: 0,
};

const TTL: Duration = Duration::from_secs(1);

impl fuser::Filesystem for WasabiFuseTest {
    fn init(&mut self, req: &Request<'_>, config: &mut KernelConfig) -> Result<(), c_int> {
        debug!("init request: config: {config:?}");
        debug!("gid: {}, uid: {}, pid: {}", req.gid(), req.uid(), req.pid());
        Ok(())
    }

    fn destroy(&mut self) {
        debug!("Shutdown");
    }

    /// Get file attributes.
    fn getattr(&mut self, _req: &Request<'_>, ino: u64, _fh: Option<u64>, reply: ReplyAttr) {
        debug!("getattr ino {ino}");
        match ino {
            1 => reply.attr(&TTL, &ROOT),
            2 => reply.attr(&TTL, &HELLO),
            _ => reply.error(ENOENT),
        }
    }

    fn readdir(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        if ino != 1 {
            reply.error(ENOENT);
            return;
        }

        let entries = vec![
            (1, FileType::Directory, "."),
            (1, FileType::Directory, ".."),
            (2, FileType::RegularFile, "hello.txt"),
        ];

        for (i, entry) in entries.into_iter().enumerate().skip(offset as usize) {
            // i + 1 means the index of the next entry
            if reply.add(entry.0, (i + 1) as i64, entry.1, entry.2) {
                break;
            }
        }
        reply.ok();
    }

    fn read(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        _size: u32,
        _flags: i32,
        _lock: Option<u64>,
        reply: ReplyData,
    ) {
        if ino == 2 {
            reply.data(&HELLO_TXT_CONTENT.as_bytes()[offset as usize..]);
        } else {
            reply.error(ENOENT);
        }
    }

    /// Look up a directory entry by name and get its attributes.
    fn lookup(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        if parent != 1 {
            warn!("parent not dir");
            reply.error(ENOTDIR);
            return;
        }

        if name != OsStr::new("hello.txt") {
            debug!("file {} does not exist", name.to_string_lossy());
            reply.error(ENOENT);
            return;
        }

        reply.entry(&TTL, &HELLO, 0);
    }
}
