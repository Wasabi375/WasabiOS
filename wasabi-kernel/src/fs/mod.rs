//! Module for filesystem related functionality
#![allow(missing_docs)]
#![allow(unused)]

use alloc::vec::Vec;
use path::{Path, PathBuf};

pub mod path;

pub enum FileSystemError {}

pub struct FileId(u64);
pub struct FileSystemId([u8; 16]);

pub struct FileInfo {
    id: FileId,
    node_type: FileType,
    path: PathBuf,
}

pub struct File {
    info: FileInfo,
}

pub struct Directory {
    info: FileInfo,
    nodes: Vec<FileInfo>,
}

pub struct Symlink {
    info: FileInfo,
    target: PathBuf,
}

pub struct MountPoint {
    file: FileInfo,
    fs_id: FileSystemId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct FileTimestamp(u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMetadata {
    created_at: FileTimestamp,
    modified_at: FileTimestamp,
    meta_timestamp: FileTimestamp,
}

pub enum FileType {
    File,
    Directory,
    Symlink,
    MountPoint,
}

pub trait Filesystem {
    fn id(&self) -> FileSystemId;

    /// Querry the file system for the id
    fn query(&mut self, id: FileId) -> Result<Option<FileInfo>, FileSystemError>;

    /// Querry the file system for the path
    fn query_path(&mut self, path: Path) -> Result<Option<FileInfo>, FileSystemError>;

    /// Open a file on the file system
    fn open(&mut self, file: FileInfo) -> Result<File, FileSystemError>;

    /// Open a directory on the file system
    fn directory(&mut self, file: FileInfo) -> Result<Directory, FileSystemError>;
}
