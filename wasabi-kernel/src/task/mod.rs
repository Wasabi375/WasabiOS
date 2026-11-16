//! A task system that allows per-core concurency
//!
//! TODO implement task stealing between cores

mod api;
mod parking;

pub use api::*;
pub use parking::Signal;

#[cfg(feature = "task-stack-history")]
mod stack_history;

use core::{
    arch::{asm, naked_asm},
    cell::UnsafeCell,
    ops::Add,
    ptr::DynMetadata,
    sync::atomic::{AtomicBool, AtomicPtr, Ordering},
};

use alloc::{boxed::Box, collections::vec_deque::VecDeque, vec::Vec};
use hashbrown::HashMap;
use log::{debug, error, info, warn};
use shared::{
    alloc_ext::{alloc_buffer_aligned, leak_allocator::LeakAllocator},
    sync::lockcell::{LockCell, LockCellGuard, LockCellInternal},
};
use shared_derive::AtomicWrapper;
use x86_64::{
    VirtAddr,
    registers::{
        control::{Cr4, Cr4Flags},
        rflags::{self, RFlags},
        xcontrol::{XCr0, XCr0Flags},
    },
    structures::{
        gdt::SegmentSelector,
        idt::{InterruptStackFrame, InterruptStackFrameValue},
        paging::Size4KiB,
    },
};

use crate::{
    DEFAULT_STACK_SIZE,
    core_local::{AutoRefCounterGuard, InterruptDisableGuard},
    cpu::{
        self,
        apic::{
            Apic,
            timer::{TimerConfig, TimerDivider, TimerMode},
        },
        cpuid::{CPUCapabilities, cpuid},
        interrupts::InterruptVector,
    },
    locals,
    mem::{MemError, page_allocator::PageAllocator, structs::GuardedPages},
    pages_required_for,
    prelude::UnwrapTicketLock,
};

/// A Task in the [TaskSystem]
struct Task {
    /// Optional debug name for the task
    name: Box<str>,

    /// the stack used by the task
    ///
    /// this is none if the task does not own the stack. Each task needs to have
    /// it's own stack, but it is possible to have a task where the memory of the
    /// stack has a lifetime that outlives the task itself.
    ///
    /// E.g. the "cpu-core" task has a stack with a static lifetime
    stack: Option<GuardedPages<Size4KiB>>,

    /// The cpu state the last time the task was interrupted
    ///
    /// This is used to resume the task at a later point.
    /// This is `None` while the task is executed
    last_interrupt: Option<TaskInterrupt>,

    /// if set [park] will immediately [unpark] and behave like [yield_now] instead.
    unpark_immediate: bool,
}

impl Drop for Task {
    fn drop(&mut self) {
        debug!("dropping task: {}", self.name);
        if let Some(stack) = self.stack.take() {
            debug_assert!(
                stack.contains(shared::read_instruction_pointer!()),
                "trying to drop stack while current ip points into it"
            );
            debug!(
                "dropping stack: {:p} - {:p}",
                stack.start_addr(),
                stack.end_addr()
            );
            unsafe {
                // Safety: Stack is no longer in use
                match stack.unmap() {
                    Ok(unmapped) => {
                        PageAllocator::get_for_kernel()
                            .lock()
                            // pages just unmapped
                            .free_guarded_pages(unmapped);
                    }
                    Err(e) => error!(
                        "Failed to unmap stack for task \"{}\". Leaking memory\n{e}",
                        self.name
                    ),
                }
            }
        }
    }
}

/// Publicly accessible data about a [Task]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskInfo {
    /// The [TaskHandle]
    pub handle: TaskHandle,
    /// A debug name for the task
    pub name: Box<str>,
}

/// Describes a Task that can be launched
#[derive(Debug, Clone)]
pub struct TaskDefinition<F> {
    /// A lambda for the task, that is executed sometime in the future
    pub task: F,

    /// A debug name for the task
    pub name: Box<str>,

    /// The stack size for the task stack.
    pub stack_size: u64,
    /// Whether to create a guard page for the stack
    pub stack_head_guard: bool,
    /// Whether to create a guard page for the stack
    pub stack_tail_guard: bool,
}

/// The debug name for the core task of each CPU
///
/// This is the task that is started directly by the bootloader/ap_startup which
/// is responsible for initializing the cpu and starting the task system
pub const CPU_CORE_TASK_NAME: &str = "cpu-core";

impl<F> TaskDefinition<F> {
    /// Create a new [TaskDefinition]
    pub const fn new<I>(task: F, name: I) -> Self
    where
        I: [const] Into<Box<str>>,
    {
        Self {
            task,
            name: name.into(),
            stack_size: DEFAULT_STACK_SIZE,
            stack_head_guard: true,
            stack_tail_guard: true,
        }
    }
}

/// A unique Handle for a [Task]
///
/// Handles are unique across cores and task systems.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, AtomicWrapper)]
#[repr(transparent)]
pub struct TaskHandle(u64);

/// the next unused [TaskHandle].
static NEXT_FREE_HANDLE: AtomicTaskHandle = AtomicTaskHandle::new(TaskHandle(1));

impl TaskHandle {
    /// A task handle used for when there is no current task.
    pub const NULL_TASK: TaskHandle = TaskHandle(0);

    /// `true` for the empty task handle that represents no task
    fn is_null_task(&self) -> bool {
        self.0 == 0
    }

    /// Returns an unused [TaskHandle]
    ///
    /// this function only produces the same value once.
    fn take_next() -> TaskHandle {
        // TODO I want to change this so I can impl this as fetch_add(1)
        NEXT_FREE_HANDLE.fetch_add(TaskHandle(1), Ordering::Relaxed)
    }

    /// Returns the inner value
    pub fn to_u64(&self) -> u64 {
        self.0
    }

    /// annotates the [TaskHandle] with the corresponding [TaskSystem]
    fn with_system(self, system: &'static TaskSystem) -> TaskHandleWithSystem {
        TaskHandleWithSystem(self, system)
    }

    /// Find the [TaskSystem] responsible for the [TaskHandle]
    ///
    /// Returns 'Err(handle)` if no [TaskSystem] controlls the handle.
    /// This can happen if the task is already terminated
    pub fn find_system(self) -> Result<TaskHandleWithSystem, TaskHandle> {
        for system in TaskSystem::iter() {
            if system.inner.lock().tasks.contains_key(&self) {
                return Ok(TaskHandleWithSystem(self, system));
            }
        }
        Err(self)
    }
}

impl Add<TaskHandle> for TaskHandle {
    type Output = TaskHandle;

    fn add(self, rhs: TaskHandle) -> Self::Output {
        TaskHandle(self.0 + rhs.0)
    }
}
impl From<TaskHandleWithSystem> for TaskHandle {
    fn from(value: TaskHandleWithSystem) -> Self {
        value.0
    }
}

/// A unique Handle for a [Task] and the [TaskSystem] that controlls the task.
///
/// Handles are unique across cores and task systems.
#[derive(Clone, Copy)]
pub struct TaskHandleWithSystem(pub TaskHandle, pub &'static TaskSystem);

impl core::fmt::Debug for TaskHandleWithSystem {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("TaskHandleWithSystem")
            .field(&self.0.0)
            .finish_non_exhaustive()
    }
}
impl core::cmp::PartialEq for TaskHandleWithSystem {
    fn eq(&self, other: &Self) -> bool {
        if self.0 == other.0 {
            assert_eq!(&raw const self.1, &raw const other.1);
            true
        } else {
            false
        }
    }
}
impl core::cmp::Eq for TaskHandleWithSystem {}
impl core::hash::Hash for TaskHandleWithSystem {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
        // TaskSystem is not required in the hash as the id is unique accross all systems
    }
}

impl core::cmp::PartialEq<TaskHandle> for TaskHandleWithSystem {
    fn eq(&self, other: &TaskHandle) -> bool {
        self.0 == *other
    }
}
impl core::cmp::PartialEq<TaskHandleWithSystem> for TaskHandle {
    fn eq(&self, other: &TaskHandleWithSystem) -> bool {
        *other == *self
    }
}

/// A task system for a cpu-core
///
/// This allows for single-core concurent task execution.
pub struct TaskSystem {
    /// the [TaskHandle] of the task currently executing on this cpu-core.
    current_task: AtomicTaskHandle,

    /// Whether the task system is running. If set to `true` the task system can interrupt
    /// the executing task at any time in order to switch to another system.
    running: AtomicBool,

    /// Used to decide whether to logger can access the data lock.
    pub log_task_locked_data: AtomicBool,

    /// Set to true when the [TaskSystem] is initialized
    ///
    /// Accessed by [CoreInfo](shared::sync::CoreInfo) to provide better debug data
    pub is_init: AtomicBool,

    /// system data that needs locked access
    inner: UnwrapTicketLock<InnerTaskSystem>,

    /// a static pointer to self
    ///
    /// # Safety:
    ///
    /// only written during init
    static_self: UnsafeCell<Option<&'static Self>>,

    /// the [Apic] associated with the [TaskSystem]
    ///
    /// This is used to access the timer and send ipis.
    /// The apic can't be accessed through `locals!()`, because it might be necessary
    /// to [`start`](Self::start) or [`pause`](Self::pause) a [TaskSystem] from a different
    /// core.
    ///
    /// # Safety:
    ///
    /// only written during init
    apic: UnsafeCell<Option<&'static UnwrapTicketLock<Apic>>>,

    /// next ptr in the task system linked list
    ///
    /// see [TASK_SYSTEMS_HEAD]
    task_system_list_next: AtomicPtr<Self>,
}

/// head ptr for the task system linked list
///
/// see [TaskSystem::task_system_list_next]
static TASK_SYSTEMS_HEAD: AtomicPtr<TaskSystem> = AtomicPtr::null();

unsafe impl Sync for TaskSystem {}

/// Data used by the [TaskSystem] that is accessed via a lock
struct InnerTaskSystem {
    /// A list of all running tasks
    tasks: HashMap<TaskHandle, Task>,

    /// a list of tasks that are currentyl interrupted and can be resumed at a future time
    interrupted: VecDeque<TaskHandle>,

    /// a list of tasks taht are currently interrupted and are waiting to resume until notified
    parked: Vec<TaskHandle>,

    /// Terminated but not yet dropped tasks.
    ///
    /// Dropping [Task] is not always save, as we need to ensure that dropping does not free
    /// the stack that is used while dropping.
    terminated: Vec<Task>,

    /// A History of stacks used by tasks and their name
    #[cfg(feature = "task-stack-history")]
    stack_history: stack_history::StackHistory,
}

impl TaskSystem {
    /// Creats a new uninitialized [TaskSystem]
    ///
    /// # Safety
    ///
    /// Caller ensures that [Self::init] is called before anything else
    pub const unsafe fn new() -> Self {
        TaskSystem {
            current_task: AtomicTaskHandle::new(TaskHandle(0)),
            running: AtomicBool::new(false),
            is_init: AtomicBool::new(false),
            log_task_locked_data: AtomicBool::new(true),
            inner: unsafe { UnwrapTicketLock::new_non_preemtable_uninit() },
            static_self: UnsafeCell::new(None),
            apic: UnsafeCell::new(None),
            task_system_list_next: AtomicPtr::null(),
        }
    }

    /// Initializes the task system
    ///
    /// This does not start the timer. use [Self::start] for that
    ///
    /// # Safety
    ///
    /// This needs to be called during process initialization, once per core
    /// and relies on a working apic.
    /// No other reference to `self` may be live during this function call.
    pub unsafe fn init(&'static self) {
        setup_xsave();

        unsafe {
            // Safety: we hold the only active reference to `self`
            *self.static_self.as_mut_unchecked() = Some(self);
        }
        // this is fine in terms of races. There shouldn't be any iterators while this is
        // initialized. And this causes no races for adding to the linked list
        // Safety-NOTE: during `init` of any `TaskSystem` it is not save to call `eq` on them
        self.task_system_list_next.store(
            TASK_SYSTEMS_HEAD.swap(self as *const _ as *mut _, Ordering::AcqRel),
            Ordering::Release,
        );

        let mut tasks = HashMap::new();
        let current_task = TaskHandle::take_next();
        tasks.insert(
            current_task,
            Task {
                name: CPU_CORE_TASK_NAME.into(),
                last_interrupt: None,
                stack: None,
                unpark_immediate: false,
            },
        );

        let mut inner = InnerTaskSystem {
            tasks,
            interrupted: VecDeque::new(),
            parked: Vec::new(),
            terminated: Vec::new(),
            #[cfg(feature = "task-stack-history")]
            stack_history: stack_history::StackHistory::new(),
        };
        #[cfg(feature = "task-stack-history")]
        inner
            .stack_history
            .register_task(locals!().stack, CPU_CORE_TASK_NAME.into());

        self.inner.lock_uninit().write(inner);
        self.current_task.store(current_task, Ordering::Release);

        self.is_init.store(true, Ordering::Release);
    }

    /// Stop the task system on panic
    ///
    /// # Safety
    ///
    /// This should only be called on panic
    pub unsafe fn panic_stop(&self) {
        let apic = self.get_apic();
        // Safety: we are panicing
        unsafe {
            LockCellInternal::<Apic>::shatter_permanent(apic);
        }
        let mut apic = apic.lock();
        let mut timer = apic.timer();
        if timer.is_running() {
            timer.stop();
        }

        self.running.store(false, Ordering::SeqCst);

        locals!()
            .task_system
            .log_task_locked_data
            .store(false, Ordering::SeqCst);

        unsafe {
            // Safety: try to prevent deadlocks during panic shutdown
            LockCellInternal::<InnerTaskSystem>::shatter_permanent(&self.inner);
        }
    }

    /// `true` if the task system is running
    ///
    /// If so the task system can interrupt
    /// the executing task at any time in order to switch to another system.
    /// Use [Self::start] and [Self::pause] to set.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    /// Returns the [TaskHandle] of the [Task] currently executing on the [TaskSystem]
    pub fn current_task(&self, order: Ordering) -> TaskHandleWithSystem {
        self.current_task.load(order).with_system(self.as_static())
    }

    /// Get some basic information about a [Task]
    pub fn get_task_info(&self, handle: impl Into<TaskHandle>) -> Option<TaskInfo> {
        let handle = handle.into();
        let inner = self.inner.lock();
        inner.tasks.get(&handle).map(|task| TaskInfo {
            handle: handle,
            name: task.name.clone(),
        })
    }

    /// Get the number of tasks in the system
    pub fn get_task_count(&self) -> usize {
        let inner = self.inner.lock();
        inner.tasks.len()
    }

    /// Enable the task system and starts the timer
    ///
    /// While running the task system can interrupt
    /// the executing task at any time in order to switch to another task.
    ///
    /// returns the interrupt vector that was previously used by the timer.
    ///
    /// This is a public API. Internal methods can use [Self::start_timer] which requires the
    /// internal lock
    pub fn start(&self) -> Option<InterruptVector> {
        self.running.store(true, Ordering::Release);
        let _guard = unsafe { InternalGuard::non_interrupt_enter() };

        info!("Starting task system");
        self.start_timer(self.inner.lock().as_ref())
    }

    /// Pauses the task system.
    ///
    /// use [Self::start] to restart it.
    ///
    /// This is a public API. Internal methods can use [Self::stop_timer] which requires the
    /// internal lock
    pub fn pause(&self) {
        let _guard = unsafe { InternalGuard::non_interrupt_enter() };

        debug!("stopping timer");
        self.stop_timer(self.inner.lock().as_ref());
    }

    /// Start/restart the APIC timer to switch between tasks
    ///
    /// returns the interrupt vector that was previously used by the timer
    fn start_timer(&self, _inner_guard: &InnerTaskSystem) -> Option<InterruptVector> {
        let mut apic = self.get_apic().lock();
        let mut timer = apic.timer();

        if timer.is_running() {
            error!("Can't start TaskSystem timer if timer is already running");
            return None;
        }
        // no need to set a handler since, ContextSwitch uses a special case handler that
        // can't be changed
        let mut old_vec = timer
            .enable_interrupt_hander(InterruptVector::ContextSwitch)
            .expect("Timer is not running");
        if let Some(int_vec) = old_vec
            && int_vec == InterruptVector::ContextSwitch
        {
            old_vec = None;
        }

        let timer_cycles = u32::try_from(timer.rate_mhz() * 1_000_000 / 1_000)
            .expect("timer duration should really fit into a u32");

        let config = TimerConfig {
            divider: TimerDivider::DivBy1,
            duration: timer_cycles, // TODO what is a good interval
        };

        timer
            .start(TimerMode::Periodic(config))
            .expect("timer properly setup");
        debug!("timer started");

        old_vec
    }

    /// Stop/pause the APIC timer
    fn stop_timer(&self, _inner_guard: &InnerTaskSystem) {
        let mut apic = self.get_apic().lock();
        let mut timer = apic.timer();
        assert_eq!(timer.vector(), Some(InterruptVector::ContextSwitch));

        timer.stop();
        debug!("timer stopped");
    }

    /// Launch the defined task
    ///
    /// # Safety
    ///
    /// the task may not terminate while references into the tasks stack exist
    pub unsafe fn launch_task<F>(
        &self,
        task_definition: TaskDefinition<F>,
    ) -> Result<TaskHandleWithSystem, MemError>
    where
        F: FnOnce() + 'static,
        F: Sized,
    {
        let stack = PageAllocator::get_for_kernel()
            .lock()
            .allocate_guarded_pages::<Size4KiB>(
                pages_required_for!(Size4KiB, task_definition.stack_size),
                task_definition.stack_head_guard,
                task_definition.stack_tail_guard,
            )?;

        let stack = unsafe {
            // Safety: stack is newly allocated and therefor not mapped
            stack.map()?
        };

        let mut launch_rflags = rflags::read();
        launch_rflags.set(RFlags::INTERRUPT_FLAG, true);
        launch_rflags.set(RFlags::OVERFLOW_FLAG, false);
        launch_rflags.set(RFlags::SIGN_FLAG, false);
        launch_rflags.set(RFlags::ZERO_FLAG, false);
        launch_rflags.set(RFlags::AUXILIARY_CARRY_FLAG, false);
        launch_rflags.set(RFlags::PARITY_FLAG, false);
        launch_rflags.set(RFlags::CARRY_FLAG, false);

        let stack_end = stack.end_addr().align_down(16u64);
        let task_size = size_of::<F>() as u64;
        let task_def_start_addr = (stack_end - task_size).align_down(align_of::<F>() as u64);

        let stack_end_minus_task = task_def_start_addr.align_down(16u64);

        unsafe {
            task_def_start_addr
                .as_mut_ptr::<F>()
                .write(task_definition.task);
        }
        let task_ptr = task_def_start_addr.as_mut_ptr::<F>();
        let task_dyn_ptr = task_ptr as *mut DynTask;
        let (task_thin, task_meta) = task_dyn_ptr.to_raw_parts();

        let mut registers = InterruptRegisterState::default();
        // rdi contains first argument
        registers.rdi = task_thin as u64;
        // rsi contains second argument
        // Safety: DynMetadata is a NonNull wrapper and therefor can be transmuted into a u64
        registers.rsi =
            unsafe { core::mem::transmute::<core::ptr::DynMetadata<_>, u64>(task_meta) };

        // TODO locals is wrong here. I need the gdt for the cpu core that the task is launched on
        let segments = locals!().gdt.kernel_segments();

        let launch_interrupt_fake = TaskInterrupt {
            stack_frame: InterruptStackFrameValue::new(
                VirtAddr::new(task_entry as *mut () as usize as u64),
                segments.code,
                launch_rflags,
                stack_end_minus_task,
                SegmentSelector::NULL,
            ),
            registers,
            xsave_area: None,
        };

        let task = Task {
            name: task_definition.name,
            stack: Some(stack),
            last_interrupt: Some(launch_interrupt_fake),
            unpark_immediate: false,
        };

        let mut inner = self.inner.lock();
        let _guard = unsafe { InternalGuard::non_interrupt_enter() };

        #[cfg(feature = "task-stack-history")]
        inner
            .stack_history
            .register_task(stack.pages, task.name.clone());

        let new_handle = TaskHandle::take_next();
        let old_task_value = inner.tasks.insert(new_handle, task);
        assert!(old_task_value.is_none());

        debug!(
            "new task {:?} stack: {:p} - {:p}",
            new_handle,
            stack.start_addr(),
            stack_end_minus_task
        );

        inner.interrupted.push_back(new_handle);

        if self.is_running()
            && let Some(old_vec) = self.start_timer(&inner)
        {
            warn!("TaskSystem is overwriting timer interrupt vector. Old vector: {old_vec}");
        }

        Ok(new_handle.with_system(self.as_static()))
    }

    /// Terminates the current task
    ///
    /// The task system will switch to another running task if one exist
    /// or halt the cpu if not.
    ///
    /// # Safety
    ///
    /// Caller must ensure that there are no active references into the stack if
    /// the task was spawned with an owned stack.
    pub unsafe fn terminate_task() -> ! {
        assert!(!locals!().in_interrupt());

        // Safety: enter context switch from outside interrupt
        let int_guard = unsafe { InternalGuard::non_interrupt_enter() };

        // task needs to terminate on the system it is running on. Therefor this is not using self
        let this = &locals!().task_system;
        let mut inner = this.inner.lock();

        let current_handle = this.current_task.load(Ordering::Acquire);
        assert!(!current_handle.is_null_task());
        let current_task = inner
            .tasks
            .remove(&current_handle)
            .expect("Current task should always exist");

        assert!(current_task.last_interrupt.is_none());
        warn!("terminate task {current_handle:?} {}", current_task.name);

        // NOTE can't drop current_task right now, this function uses the stack
        // owned by it. Dropping it now would also unmap the stack. Instead we
        // keep track of terminated tasks and drop them later.
        inner.terminated.push(current_task);

        this.resume_next_task(inner, int_guard);
    }

    /// Called by the park context-switch interrupt handler
    fn task_parked(interrupt: TaskInterrupt, int_guard: InternalGuard) -> ! {
        // Called from interrupt. This needs to be the TaskSystem for the current cpu
        let this = &locals!().task_system;

        let mut inner = this.inner.lock();

        let current_handle = this.current_task.load(Ordering::Acquire);
        assert!(!current_handle.is_null_task());

        debug_assert!(verify_interrupt_frame(
            interrupt.stack_frame,
            &inner.tasks[&current_handle],
            &inner
        ));

        let current_task = inner
            .tasks
            .get_mut(&current_handle)
            .expect("Current task should always exist");

        assert!(current_task.last_interrupt.is_none());
        current_task.last_interrupt = Some(interrupt);

        if current_task.unpark_immediate {
            current_task.unpark_immediate = false;
            inner.interrupted.push_back(current_handle);
        } else {
            inner.parked.push(current_handle);
        }

        this.resume_next_task(inner, int_guard);
    }

    /// resumes the next task
    fn resume_next_task<L: LockCell<InnerTaskSystem>>(
        &self,
        mut inner: LockCellGuard<'_, InnerTaskSystem, L>,
        int_guard: InternalGuard,
    ) -> ! {
        if let Some(next_task_handle) = inner.interrupted.pop_front() {
            self.resume_task(inner, next_task_handle, int_guard);
        } else {
            debug!("No tasks ready to resume. Halting");
            // pause the system
            self.stop_timer(inner.as_ref());
            self.current_task
                .store(TaskHandle::NULL_TASK, Ordering::Release);
            drop(inner);

            // reenable interrupts befor halt
            drop(int_guard);
            cpu::halt();
        }
    }

    /// Called by the context-switch interrupt handler
    fn task_interrupted(interrupt: TaskInterrupt, int_guard: InternalGuard) -> ! {
        // Called from interrupt. This needs to be the TaskSystem for the current cpu
        let this = &locals!().task_system;

        let mut inner = this.inner.lock();

        // TODO this is not the best time to drop terminated tasks. Instead I want
        // to have some kind of cleanup task. But this works for now
        // TODO I should be able to change PageTable/PageAlloc/PhysAlloc locks to be preemtable
        // once this is no longer in the context-switch path
        // inner.terminated.clear();

        let current_handle = this.current_task.load(Ordering::Acquire);

        // The "null task" can be "interrupted" after the timer is stopped,
        // if park/terminate stops the last active task. Then a different task
        // is unparked and this produces the null-task here on the next timer tick.
        if !current_handle.is_null_task() {
            debug_assert!(verify_interrupt_frame(
                interrupt.stack_frame,
                &inner.tasks[&current_handle],
                &inner
            ));

            let current_task = inner
                .tasks
                .get_mut(&current_handle)
                .expect("Current task should always exist");

            assert!(current_task.last_interrupt.is_none());
            current_task.last_interrupt = Some(interrupt);

            inner.interrupted.push_back(current_handle);
        }

        let next_task_handle = inner
            .interrupted
            .pop_front()
            .expect("This should always at least contain the currently interrupted task");
        this.resume_task(inner, next_task_handle, int_guard);
    }

    /// resumes a [Task]
    fn resume_task<L: LockCell<InnerTaskSystem>>(
        &self,
        mut inner: LockCellGuard<'_, InnerTaskSystem, L>,
        next_task_handle: TaskHandle,
        int_guard: InternalGuard,
    ) -> ! {
        let next_task = inner
            .tasks
            .get_mut(&next_task_handle)
            .expect("Task should always exist if we find the handle");

        let Some(next_task_interrupt) = next_task.last_interrupt.take() else {
            panic!("next task was never iterrupted");
        };
        self.current_task.store(next_task_handle, Ordering::Release);

        drop(inner);

        next_task_interrupt.resume(int_guard)
    }

    /// returns a `'static` reference to self
    ///
    /// this is possible because `TaskSystem` can only be created in static memory.
    #[inline(always)]
    pub fn as_static<'a>(&'a self) -> &'static Self {
        unsafe {
            // Safety: `init` was called an therefor this is a valid reference.
            // see [Self::new] and [Self::init]
            self.static_self
                .as_ref_unchecked()
                .expect("get_static_self must not be called before init")
        }
    }

    /// Get access to the apic of this [TaskSystem].
    #[inline(always)]
    fn get_apic(&self) -> &UnwrapTicketLock<Apic> {
        unsafe {
            // Safety: `init` was called an therefor this is a valid reference.
            // see [Self::new] and [Self::init]
            self.apic
                .as_ref_unchecked()
                .expect("get_apic must not be called before init")
        }
    }

    /// Iterate over all [TaskSystem]
    pub fn iter() -> impl Iterator<Item = &'static TaskSystem> {
        let next_ptr = TASK_SYSTEMS_HEAD.load(Ordering::Acquire);
        if next_ptr.is_null() {
            TaskSystemIter(None)
        } else {
            TaskSystemIter(unsafe {
                // Safety: next_ptr is initialized to a valid static during `init`.
                // Therefor this is either null (checked above) or valid.
                Some(&*next_ptr)
            })
        }
    }
}

impl PartialEq for TaskSystem {
    fn eq(&self, other: &Self) -> bool {
        // Task systems are only created in static memory and contain a static reference to self.
        // Therefor it is enough to check that the `static_self` reference of both sides are equal

        self.as_static() as *const _ == other.as_static() as *const _
    }
}

#[derive(Clone, Copy)]
struct TaskSystemIter(Option<&'static TaskSystem>);

impl Iterator for TaskSystemIter {
    type Item = &'static TaskSystem;

    fn next(&mut self) -> Option<Self::Item> {
        let Some(current) = self.0 else {
            return None;
        };

        let next_ptr = current.task_system_list_next.load(Ordering::Acquire);
        self.0 = if next_ptr.is_null() {
            None
        } else {
            unsafe {
                // Safety: next_ptr is initialized to a valid static during `init`.
                // Therefor this is either null (checked above) or valid.
                Some(&*next_ptr)
            }
        };
        return Some(current);
    }
}

fn verify_interrupt_frame<L: LockCell<InnerTaskSystem>>(
    stack_frame: InterruptStackFrameValue,
    task: &Task,
    inner: &LockCellGuard<'_, InnerTaskSystem, L>,
) -> bool {
    let Some(stack) = task.stack else { return true };

    let stack_ptr = stack_frame.stack_pointer;
    if stack.pages.contains(stack_ptr) {
        return true;
    }

    error!(
        "interrupt with stack pointer not pointing into the stack of it's task!
        task name: {}
        stack ptr: {:p}
        stack: {:p}-{:p}
        instruction ptr: {:p}",
        task.name,
        stack_ptr,
        stack.start_addr(),
        stack.end_addr(),
        stack_frame.instruction_pointer,
    );

    #[cfg(feature = "task-stack-history")]
    if let Some((stack, owner)) = inner.stack_history.find(stack_ptr) {
        error!(
            "It looks like the stack pointer, points into the stack of another task
            interrupted task: {}
            stack owner: {}
            stack pointer: {:p}
            stack: {:p}-{:p}
            instruction ptr: {:p}",
            task.name,
            owner,
            stack_ptr,
            stack.start_addr(),
            stack.end_addr(),
            stack_frame.instruction_pointer,
        );
    } else {
        error!(
            "Could not find stack for stack pointer.
            interrupted task: {}
            stack pointer: {:p}
            instruction ptr: {:p}",
            task.name, stack_ptr, stack_frame.instruction_pointer,
        );
    }

    false
}

/// The state of an interrupted [Task]
///
/// can be used to [Self::resume] a [Task]
struct TaskInterrupt {
    stack_frame: InterruptStackFrameValue,
    registers: InterruptRegisterState,
    xsave_area: Option<Box<[u8]>>,
}

impl TaskInterrupt {
    /// resumes the interrupted task
    fn resume(self, mut int_guard: InternalGuard) -> ! {
        // NOTE: logging here causes deadlock
        #[inline]
        extern "C" fn dealloc_xsave_area(ptr: *mut u8, len: usize) {
            unsafe {
                // Safety: ptr and len refer to Box<[u8]> that we own
                let _ = Box::from_raw(core::ptr::slice_from_raw_parts_mut(ptr, len));
            }
        }

        #[inline]
        extern "C" fn drop_guard(guard: *mut InternalGuard) {
            unsafe {
                // Safety: ensure to keep track of interrupt counters
                // drop manually as the assembly exits the function without automatically dorpping
                (*guard).keep_ints_disabled();
                // Safety: guard no longer used after "resume" and we are calling
                // this from noreturn asm
                core::ptr::drop_in_place(guard);
            }
        }

        let (xsave_area_size, xsave_area) = if let Some(xsave_area) = self.xsave_area {
            (xsave_area.len(), Box::leak(xsave_area).as_mut_ptr())
        } else {
            (0, core::ptr::null_mut())
        };
        assert!(xsave_area.is_aligned_to(64));

        unsafe {
            // Safety: this has to be done in assembly
            asm!(
                // push fields for iretq
                "push {stack_segment:r}",
                "push {new_stack_pointer}",
                "push {rflags}",
                "push {code_segment:r}",
                "push {new_instruction_pointer}",

                // push registers, because there is a chance
                // it gets clobberd by any function we call
                "push {registers}",


                // restore xsave state if available
                "test rdi, rdi",
                "jz 2f",
                // xsave requested-feature bitmap of all 1 to restore all features
                // rdi contains the xsave_area, eax, rdx are already set to -1 for rfbm
                "xrstor64 [rdi]",
                // free memory used for xsave area
                // arguments already loaded into rsi, rdi
                "call {dealloc_xsave}",
                "2:",

                "mov rdi, {guard_ptr}
                call {drop_guard}",

                // ensure that this task switch is not interupted between eoi and iretq
                "cli",

                // issue end of interrupt
                // TODO is this needed in all cases?
                "call {eoi}",

                // pop registers value into rax to restore any clobber from previous function calls
                "pop rax",

                // restore registers, except for rax
                "
                mov rbx, [rax + 0x8]
                mov rcx, [rax + 0x10]
                mov rdx, [rax + 0x18]
                mov rsi, [rax + 0x20]
                mov rdi, [rax + 0x28]
                mov rbp, [rax + 0x30]
               
                mov r8 , [rax + 0x38]
                mov r9 , [rax + 0x40]
                mov r10, [rax + 0x48]
                mov r11, [rax + 0x50]
                mov r12, [rax + 0x58]
                mov r13, [rax + 0x60]
                mov r14, [rax + 0x68]
                mov r15, [rax + 0x70]
                ",
                // restore rax last
                "mov rax, [rax]",


                "iretq",

                in("rax") -1,
                in("rdx") -1,
                in("rdi") xsave_area,
                in("rsi") xsave_area_size,

                dealloc_xsave = sym dealloc_xsave_area,
                drop_guard = sym drop_guard,

                eoi = sym Apic::eoi,

                registers = in(reg) &self.registers as *const _,

                rflags = in(reg) self.stack_frame.cpu_flags.bits(),
                new_instruction_pointer = in(reg) self.stack_frame.instruction_pointer.as_u64(),
                new_stack_pointer = in(reg) self.stack_frame.stack_pointer.as_u64(),
                code_segment = in(reg) self.stack_frame.code_segment.0,
                stack_segment = in(reg) self.stack_frame.stack_segment.0,

                guard_ptr = in(reg) &mut int_guard,

                clobber_abi("C"),
                options(noreturn)
            )
        }
    }
}

/// Guards general access to TaskSystem internals
///
/// this includes
/// * inc/dec interrupt count (if applicable)
/// * en/disable interrupts (if necessary)
/// * stop logger from accessing task state (deadlock prevention)
struct InternalGuard {
    _int_count_guard: Option<AutoRefCounterGuard<'static>>,
    int_disable_guard: Option<InterruptDisableGuard>,
}

impl InternalGuard {
    /// Interrupt leading to context switch entered TaskSystem
    ///
    /// # Safety
    ///
    /// must be called once in interrupts in code paths that lead to [TaskInterrupt::resume]
    /// before [SystemData] lock is taken to ensure context-switch is not preemted
    /// by another context-switch
    #[must_use]
    unsafe fn interrupt_enter() -> Self {
        locals!()
            .task_system
            .log_task_locked_data
            .store(false, Ordering::Relaxed);

        Self {
            _int_count_guard: Some(locals!().interrupt_count.increment()),
            int_disable_guard: None,
        }
    }

    /// Enter a context switch without an existing interrupt
    ///
    /// # Safety
    ///
    /// must be called once  in *non* interrupts in code paths that lead to [TaskInterrupt::resume]
    /// before [SystemData] lock is taken to ensure context-switch is not preemted
    /// by another context-switch
    #[must_use]
    unsafe fn non_interrupt_enter() -> Self {
        locals!()
            .task_system
            .log_task_locked_data
            .store(false, Ordering::Relaxed);

        let disable = unsafe { locals!().disable_interrupts() };
        Self {
            _int_count_guard: None,
            int_disable_guard: Some(disable),
        }
    }

    /// Ensures that dropping this does not reenable interrupts
    ///
    /// # Safety
    ///
    /// Caller ensures that it is save to keep interrupts disabled.
    /// Caller ensures that the interrupt flag is properly set sometime after this call
    ///
    /// There is a high chance of race conditions when using this, as random functions might
    /// reenable interrupts before the caller expects them to be enabled,
    ///     e.g any memory alloc/free uses a lock which will potentially modify the Interrupt flag
    unsafe fn keep_ints_disabled(&mut self) {
        if let Some(guard) = self.int_disable_guard.take() {
            core::mem::forget(guard);
            unsafe {
                // Safety: we just dropped the guard
                locals!().decrement_interrupt_disable_count();
            }
        }
    }
}

impl Drop for InternalGuard {
    fn drop(&mut self) {
        locals!()
            .task_system
            .log_task_locked_data
            .store(true, Ordering::Relaxed);
    }
}

/// the number of pages used for context swtich interrupts
///
/// Do I need this stack? I am not really sure
pub const CONTEXT_SWITCH_STACK_PAGE_COUNT: u64 = 8;

/// Naked assembly for a context-switch interrupt handler that calls `$rust_handler`
///
/// ```
/// #[inline]
/// extern "C" fn rust_handler(
///     stack_frame: &InterruptStackFrame,
///     registers: &InterruptRegisterState,
///     xsave_area: *mut u8,
/// ) -> !
/// ```
macro_rules! context_switch_interrupt_handler {
    ($(#[$meta:meta])* $vis:vis fn $interrupt_handler_name:ident($stack_frame:ident: &InterruptStackFrame, $registers:ident: &InterruptRegisterState, $xsave_area:ident: *mut u8) -> ! $block:block) => {
        $(#[$meta])*
        #[unsafe(naked)]
        $vis extern "x86-interrupt" fn $interrupt_handler_name(_int_stack_frame: InterruptStackFrame) {
            #[inline]
            extern "C" fn __rust_handler(
                $stack_frame: &InterruptStackFrame,
                $registers: &InterruptRegisterState,
                $xsave_area: *mut u8,
            ) -> ! {
                $block
            }

            #[inline]
            extern "C" fn __alloc_xsave_area() -> *const u8 {
                let xsave_area_size = cpuid(0xd, None).ebx as usize;

                // TODO is there a way I can deterministically alloc the xsave area?
                // I don't really want the interrupt here have to allocate every time
                let xsave_area =
                    alloc_buffer_aligned(xsave_area_size, 64).expect("Failed to allocate xsave area");
                Box::leak(xsave_area).as_ptr()
            }

            naked_asm!(
                "cld", // clear direction flag used by string operations
                // store registers, except rsp(stack) which is part of the InterruptStackFrame
                "
                push r15
                push r14
                push r13
                push r12
                push r11
                push r10
                push r9
                push r8
                push rbp
                push rdi
                push rsi
                push rdx
                push rcx
                push rbx
                push rax
                ",
                // allocate xsave area
                "call {alloc_xsave}",
                "mov r12, rax",

                // set xsave requested-feature bitmap to all 1 (use all features)
                "mov eax, -1",
                "mov edx, -1",
                // save feature states to xsave area
                "xsave64 [r12]",

                // call rust handler
                "lea rdi, [rsp + {stack_frame_offset}]", // load stack-frame addr into rdi (first arg)
                "lea rsi, [rsp + {registers_offset}]", // load register-state addr into rsi (second arg)
                "mov rdx, r12",         // move xsave area into rdx (third arg).
                "call {rust_handler}",
                // somethings gone wrong if this is executed
                "
                int3
                int3
                int3
                hlt
                ",
                alloc_xsave = sym __alloc_xsave_area,
                rust_handler = sym __rust_handler,
                stack_frame_offset = const size_of::<$crate::task::InterruptRegisterState>(),
                registers_offset = const 0x0,
            );
        }
    };
}

context_switch_interrupt_handler! {
    /// The context-switch interrupt handler used by the [TaskSystem] timer
    pub fn context_switch_handler(
        stack_frame: &InterruptStackFrame,
        registers: &InterruptRegisterState,
        xsave_area: *mut u8
    ) -> ! {
        // Safety: start of context_switch_handler
        let int_guard = unsafe { InternalGuard::interrupt_enter() };

        let xsave_area_size = cpuid(0xd, None).ebx as usize;
        let xsave_area = unsafe {
            // Safety: alloc_xsave_area allocated this ptr as a box
            Box::from_raw(core::ptr::slice_from_raw_parts_mut(
                xsave_area,
                xsave_area_size,
            ))
        };

        assert!(stack_frame.cpu_flags.contains(RFlags::INTERRUPT_FLAG));

        let interrupt = TaskInterrupt {
            stack_frame: **stack_frame,
            registers: registers.clone(),
            xsave_area: Some(xsave_area),
        };
        TaskSystem::task_interrupted(interrupt, int_guard);
    }
}

context_switch_interrupt_handler! {
    /// The context-switch interrupt handler used by the [park] for the [TaskSystem]
    pub fn park_task_handler(
        stack_frame: &InterruptStackFrame,
        registers: &InterruptRegisterState,
        xsave_area: *mut u8
    ) -> ! {
        // Safety: start of context_switch_handler
        let int_guard = unsafe { InternalGuard::interrupt_enter() };

        let xsave_area_size = cpuid(0xd, None).ebx as usize;
        let xsave_area = unsafe {
            // Safety: alloc_xsave_area allocated this ptr as a box
            Box::from_raw(core::ptr::slice_from_raw_parts_mut(
                xsave_area,
                xsave_area_size,
            ))
        };

        assert!(stack_frame.cpu_flags.contains(RFlags::INTERRUPT_FLAG));

        let interrupt = TaskInterrupt {
            stack_frame: **stack_frame,
            registers: registers.clone(),
            xsave_area: Some(xsave_area),
        };

        TaskSystem::task_parked(interrupt, int_guard)
    }
}

/// State of all basic registers
#[repr(C)]
#[derive(Debug, Clone, Default)]
struct InterruptRegisterState {
    rax: u64,
    rbx: u64,
    rcx: u64,
    rdx: u64,
    rsi: u64,
    rdi: u64,
    rbp: u64,

    r8: u64,
    r9: u64,
    r10: u64,
    r11: u64,
    r12: u64,
    r13: u64,
    r14: u64,
    r15: u64,
}

/// setup necessary to enable the xsave/xrstor assembly instructions
fn setup_xsave() {
    let cpu_cap = CPUCapabilities::get();
    assert!(cpu_cap.xsave, "XSAVE instruction not supported");

    unsafe {
        // Safety: xsave feature flag set in cpuid therefor this should be ok
        Cr4::update(|cr4| cr4.set(Cr4Flags::OSXSAVE, true));
    }

    // for now, just save every feature set that can be saved when using xsave
    let all_features = cpu_cap.xsave_features;
    assert!(all_features.contains(XCr0Flags::X87));
    assert!(all_features.contains(XCr0Flags::SSE));
    assert!(all_features.contains(XCr0Flags::AVX));

    unsafe {
        // Safety: all_features is supported based on the cpuid result
        XCr0::write(*all_features);
    }

    let xsave_area_size = cpuid(0xd, None).ebx as usize;
    info!("xsave area size: {0}|{0:#x} bytes", xsave_area_size);
}

type DynTask = dyn FnOnce();

// unsafe fn task_entry(task: Box<dyn FnOnce() -> ()>) -> ! {
unsafe fn task_entry(task_ptr: *mut (), task_dyn: DynMetadata<DynTask>) -> ! {
    let task_ptr: *mut DynTask = core::ptr::from_raw_parts_mut(task_ptr, task_dyn);
    let task = unsafe {
        // Safety: task_ptr is valid and LeakAllocator can be used for any allocation
        Box::from_raw_in(task_ptr, LeakAllocator::get())
    };
    task();

    unsafe {
        // Safety: this is not save. Task might leak stack data
        TaskSystem::terminate_task();
    }
}
