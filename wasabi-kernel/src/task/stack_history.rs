use alloc::collections::vec_deque::VecDeque;
use x86_64::{VirtAddr, structures::paging::Size4KiB};

use crate::mem::structs::Pages;

#[derive(Debug, Clone)]
pub struct StackHistory {
    history: VecDeque<(Pages<Size4KiB>, &'static str)>,
    max_capacity: usize,
}

impl StackHistory {
    pub const fn new() -> Self {
        Self::with_max_capacity(1000)
    }

    pub const fn with_max_capacity(max_capacity: usize) -> Self {
        Self {
            history: VecDeque::new(),
            max_capacity,
        }
    }

    pub fn register_task(&mut self, stack: Pages<Size4KiB>, name: &'static str) {
        self.history.push_front((stack, name));

        self.trim_to_max();
    }

    pub fn iter(&self) -> impl Iterator<Item = (Pages<Size4KiB>, &'static str)> {
        self.history.iter().cloned()
    }

    pub fn find(&self, vaddr: VirtAddr) -> Option<(Pages<Size4KiB>, &'static str)> {
        self.iter().find(|(stack, _)| stack.contains(vaddr))
    }

    fn trim_to_max(&mut self) {
        while self.history.len() > self.max_capacity {
            let _ = self.history.pop_back();
        }
    }
}
