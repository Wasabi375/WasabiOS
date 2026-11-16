//! The public task api

use core::sync::atomic::Ordering;

use shared::sync::lockcell::{LockCell, LockCellGuard};

use crate::{
    cpu::{
        apic::ipi::{DeliveryMode, Destination, Ipi},
        interrupts::InterruptVector,
    },
    locals,
    task::{InnerTaskSystem, TaskHandleWithSystem},
};

#[expect(unused_imports)] // used for docs
use super::{Task, TaskSystem};

/// Cooperatively gives up the execution to the [TaskSystem]
#[inline]
pub fn yield_now() {
    let mut apic = locals!().apic.lock();
    apic.send_ipi(Ipi {
        mode: DeliveryMode::Fixed(InterruptVector::ContextSwitch),
        destination: Destination::SelfOnly,
    });
}

/// Get a handle to the current [Task]
#[inline]
pub fn current() -> TaskHandleWithSystem {
    locals!().task_system.current_task(Ordering::Acquire)
}

/// Park the current task until it is unparked
///
/// # `park` and `unpark`
///
/// Every [Task] has some basic blocking functionallity. [`park`] blocks the current [Task]
/// which can be resumed from another [Task] using [`unpark`].
///
/// Conceptually, each [Task] has an associated token, which initialy is not present.
///
/// * the [`park`] function blocks the current [Task] unless or untill the token is available for
///   its [TaskHandle], at which point it is consumed and the task continues. It may also *spuriously*
///   continue without consuming the token.
///
/// * [`unpark`] will make the token available for a [Task] if it wasn't already. A Task can hold
///   the token even when not currently parked so a call to [`park`] followed by a call to [`unpark`]
///   behaves just like [yield_now].
///   
///   However, note that to rely on this guarantee, you need to make sure that your unpark happens
///   after all park that may be done by other data structures!
///
/// * [`clear_unpark`] removes the token if one is available without interrupting execution like
///   [park] or [yield_now]
///
/// see [unpark]
#[inline]
pub fn park() {
    let mut apic = locals!().apic.lock();
    apic.send_ipi(Ipi {
        mode: DeliveryMode::Fixed(InterruptVector::ParkTask),
        destination: Destination::SelfOnly,
    });
    core::sync::atomic::fence(Ordering::Acquire);
}

/// unparks a [Task]
///
/// see [park]
pub fn unpark(task: TaskHandleWithSystem) {
    let system = task.1;

    let mut inner = system.inner.lock();
    if let Some(index) = inner.parked.iter().position(|t| *t == task) {
        inner.parked.swap_remove(index);
        inner.interrupted.push_back(task.into());

        if system.current_task(Ordering::Acquire).0.is_null_task() {
            system.start_timer(&inner);
        }
    } else if let Some(task) = inner.tasks.get_mut(&task.0) {
        task.unpark_immediate = true;
    }

    core::sync::atomic::fence(Ordering::Release);
}

/// unparks all [Tasks](Task)
///
/// this is a more efficient version of [unpark] for multiple tasks.
///
/// see [park]
pub fn unpark_all<I: IntoIterator<Item = TaskHandleWithSystem>>(tasks: I) {
    let mut current_sytem: Option<(&'static TaskSystem, LockCellGuard<InnerTaskSystem, _>)> = None;
    let mut timer_started = false;

    for TaskHandleWithSystem(task, system) in tasks {
        if !matches!(current_sytem, Some((locked_system, _)) if locked_system == system) {
            current_sytem = Some((system.as_static(), system.inner.lock()));
            timer_started = false;
        }

        let (_, inner) = current_sytem.as_mut().expect("initialized above");
        if let Some(index) = inner.parked.iter().position(|t| *t == task) {
            inner.parked.swap_remove(index);
            inner.interrupted.push_back(task);

            if !timer_started && system.current_task(Ordering::Acquire).0.is_null_task() {
                system.start_timer(&inner);
                timer_started = true;
            }
        } else if let Some(task) = inner.tasks.get_mut(&task) {
            task.unpark_immediate = true;
        }
    }
    core::sync::atomic::fence(Ordering::Release);
}

/// clears the unpark-immediate ticket set by [unpark]
///
/// see [park]
pub fn clear_unpark() {
    let system = &locals!().task_system;
    let mut inner = system.inner.lock();
    let current_handle = system.current_task.load(Ordering::Acquire);

    let mut task = inner.tasks.get_mut(&current_handle);
    let Some(task) = task.as_mut() else {
        if current_handle.is_null_task() {
            return;
        } else {
            panic!("current task should always exist");
        }
    };
    task.unpark_immediate = false;
}
