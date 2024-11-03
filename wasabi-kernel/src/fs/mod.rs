//! Module for filesystem related functionality
#![allow(missing_docs)]

use alloc::vec::Vec;
use path::{Path, PathBuf};

pub mod path;

pub enum FileSystemError {}

pub struct INodeId([u8; 16]);
pub struct FileSystemId([u8; 16]);

pub struct INode {
    id: INodeId,
    node_type: INodeType,
    path: PathBuf,
}

pub struct File {
    inode: INode,
}

pub struct Directory {
    inode: INode,
    nodes: Vec<INode>,
}

pub struct Symlink {
    inode: INode,
    target: PathBuf,
}

pub struct MountPoint {
    inode: INode,
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

pub enum INodeType {
    File,
    Directory,
    Symlink,
    MountPoint,
}

pub trait Filesystem {
    fn id(&self) -> FileSystemId;

    /// Querry the file system for the inode id
    fn query(&mut self, id: INodeId) -> Result<Option<INode>, FileSystemError>;

    /// Querry the file system for the path
    fn query_path(&mut self, path: Path) -> Result<Option<INode>, FileSystemError>;

    /// Open a file on the file system
    fn open(&mut self, inode: INode) -> Result<File, FileSystemError>;

    /// Open a directory on the file system
    fn directory(&mut self, inode: INode) -> Result<Directory, FileSystemError>;
}
