#![no_std]
#![allow(unused)] // TODO temp

// FIXME: StaticVec and StaticString have 2 problems that I need to fix
//   1. It uses usize for the length, wasting a lot of memory
//   2. It has a undetermined memory layout

use core::marker::PhantomData;

use static_assertions::const_assert;
use staticvec::{StaticString, StaticVec};

extern crate alloc;

#[repr(transparent)]
struct LBA(u64);

#[repr(transparent)]
struct INode(u64);

#[repr(u8)]
enum INodeType {
    File,
    Directory,
}

#[repr(C)]
struct Timestamp(u64);

const I_NODE_INLINE_BLOCK_ADDRS: usize = 3;
const I_NODE_MAX_NAME_LEN: usize = 40;
#[repr(C)]
struct INodeData {
    inode: INode,
    parent: INode,
    typ: INodeType,
    created_at: Timestamp,
    modified_at: Timestamp,
    name: StaticString<I_NODE_MAX_NAME_LEN>,
    total_block_count: u32,
    block_counts: [u16; I_NODE_INLINE_BLOCK_ADDRS],
    block_addrs: [LBA; I_NODE_INLINE_BLOCK_ADDRS],
}

#[repr(transparent)]
struct NodePointer(LBA);

#[repr(C)]
struct MainHeader {
    uuid: [u8; 16],
    root: NodePointer,
}
const_assert!(size_of::<MainHeader>() <= 512);

#[repr(C)]
enum TreeNode {
    Leave {
        parent: NodePointer,
        nodes: StaticVec<INodeData, 3>,
    },
    Node {
        parent: NodePointer,
        children: StaticVec<(INode, NodePointer), 30>,
    },
}
const_assert!(size_of::<TreeNode>() <= 512);
