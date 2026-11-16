//! Module containing structs and functionality for parking and unparking tasks

use core::mem;

use alloc::collections::{TryReserveError, VecDeque};

use crate::{
    prelude::TicketLock,
    task::{TaskHandle, TaskHandleWithSystem, unpark, unpark_all},
};

use shared::sync::lockcell::LockCell;

#[expect(unused_imports)] // for docs
use super::Task;

/// A signal allows multiple tasks to wait until the signal is triggered
pub struct Signal {
    waiting: TicketLock<VecDeque<TaskHandleWithSystem>>,
    notify_on_drop: bool,
}

impl Signal {
    /// Creates a new [Signal]
    pub fn new() -> Self {
        Self {
            waiting: TicketLock::new(VecDeque::new()),
            notify_on_drop: false,
        }
    }

    /// if set to `true` all waiting tasks will be notified if/when the [Signal] is dropped
    pub fn set_notify_on_drop(&mut self, notify_on_drop: bool) {
        self.notify_on_drop = notify_on_drop;
    }

    /// returns wether all waiting tasks will be notified if/when the [Signal] is dropped
    pub fn notify_on_drop(&self) -> bool {
        self.notify_on_drop
    }

    /// Register a [TaskHandle] as waiting
    ///
    /// tasks are only notified once, so they are automatically unregistered when notified.
    ///
    /// see [Self::unregister]
    pub fn register(&self, task: TaskHandleWithSystem) -> Result<(), TryReserveError> {
        let mut waiting = self.waiting.lock();
        waiting.try_reserve(1)?;

        waiting.push_back(task);

        Ok(())
    }

    /// Unregister a [TaskHandle] from the signal.
    ///
    /// Returns `true` if the [TaskHandle] was registered.
    pub fn unregister(&self, task: impl Into<TaskHandle>) -> bool {
        let task = task.into();
        let mut waiting = self.waiting.lock();

        if let Some(index) = waiting.iter().position(|waiting| *waiting == task) {
            waiting.swap_remove_back(index);

            return true;
        } else {
            return false;
        }
    }

    /// Notifies the first [Task] and [unparks](unpark) it
    pub fn notify_one(&self) {
        let mut waiting = self.waiting.lock();

        let Some(task) = waiting.pop_front() else {
            return;
        };

        unpark(task);
    }

    /// Notifies all [Tasks](Task) and [unparks](unpark_all) them
    pub fn notify_all(&self) {
        let tasks = mem::take(self.waiting.lock().as_mut());

        unpark_all(tasks);
    }
}
