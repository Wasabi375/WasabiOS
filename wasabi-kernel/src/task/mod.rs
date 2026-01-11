//! A task system that allows per-core concurency
//!
//! TODO implement task stealing between cores

#[cfg(feature = "task-stack-history")]
mod stack_history;

use core::{
    arch::{asm, naked_asm},
    ops::Add,
    ptr::DynMetadata,
    sync::atomic::{AtomicBool, Ordering},
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
    structures::{gdt::SegmentSelector, idt::InterruptStackFrame, paging::Size4KiB},
};

use crate::{
    DEFAULT_STACK_SIZE,
    core_local::{AutoRefCounterGuard, InterruptDisableGuard},
    cpu::{
        self,
        apic::{
            Apic,
            timer::{TimerConfig, TimerDivider, TimerError, TimerMode},
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
    name: Option<&'static str>,

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
}

impl Drop for Task {
    fn drop(&mut self) {
        if let Some(name) = self.name {
            debug!("dropping task: {name}");
        } else {
            debug!("dropping unnamed task");
        }
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
                        "Failed to unmap stack for task {}. Leaking memory\n{e}",
                        self.name.unwrap_or("")
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
    pub name: Option<&'static str>,
}

/// Describes a Task that can be launched
#[derive(Debug, Clone)]
pub struct TaskDefinition<F> {
    /// A lambda for the task, that is executed sometime in the future
    pub task: F,

    /// A debug name for the task
    pub debug_name: Option<&'static str>,

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
pub const CPU_CORE_TASK_NAME: &'static str = "cpu-core";

impl<F> TaskDefinition<F> {
    /// Create a new [TaskDefinition]
    pub const fn new(task: F) -> Self {
        Self {
            task,
            debug_name: None,
            stack_size: DEFAULT_STACK_SIZE,
            stack_head_guard: true,
            stack_tail_guard: true,
        }
    }

    /// Create a new [TaskDefinition]
    pub const fn with_name(task: F, name: &'static str) -> Self {
        Self {
            task,
            debug_name: Some(name),
            stack_size: DEFAULT_STACK_SIZE,
            stack_head_guard: true,
            stack_tail_guard: true,
        }
    }
}

/// A unique Handle for a [Task]
///
/// Handles are unique across cores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, AtomicWrapper)]
pub struct TaskHandle(u64);

impl Add<TaskHandle> for TaskHandle {
    type Output = TaskHandle;

    fn add(self, rhs: TaskHandle) -> Self::Output {
        TaskHandle(self.0 + rhs.0)
    }
}

/// the next unused [TaskHandle].
static NEXT_FREE_HANDLE: AtomicTaskHandle = AtomicTaskHandle::new(TaskHandle(0));

impl TaskHandle {
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

    /// Whether the interrupt timer is running.
    /// The timer might be disabled if there are currently no tasks interrupted tasks available.
    /// However the timer can be restarted any time as long as [Self::running] is `true`.
    timer_running: AtomicBool,

    /// Used to decide whether to logger can access the data lock.
    pub log_task_locked_data: AtomicBool,

    /// Set to true when the [TaskSystem] is initialized
    ///
    /// Accessed by [CoreInfo](shared::sync::CoreInfo) to provide better debug data
    pub is_init: AtomicBool,

    /// system data that needs locked access
    data: UnwrapTicketLock<SystemData>,
}

/// Data used by the [TaskSystem] that is accessed via a lock
struct SystemData {
    /// A list of all running tasks
    tasks: HashMap<TaskHandle, Task>,

    /// a list of tasks that are currentyl interrupted and can be resumed at a future time
    interrupted: VecDeque<TaskHandle>,

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
            timer_running: AtomicBool::new(false),
            is_init: AtomicBool::new(false),
            log_task_locked_data: AtomicBool::new(true),
            data: unsafe { UnwrapTicketLock::new_non_preemtable_uninit() },
        }
    }

    /// Initializes the task system
    ///
    /// This does not start the timer. use [Self::start] for that
    ///
    /// # Safety
    ///
    /// This needs to be called during process initialization, once per core
    /// and relies on a working apic
    pub unsafe fn init(&self) {
        setup_xsave();

        let mut tasks = HashMap::new();
        let current_task = TaskHandle::take_next();
        tasks.insert(
            current_task,
            Task {
                name: Some(CPU_CORE_TASK_NAME),
                last_interrupt: None,
                stack: None,
            },
        );

        let mut system_data = SystemData {
            tasks,
            interrupted: VecDeque::new(),
            terminated: Vec::new(),
            #[cfg(feature = "task-stack-history")]
            stack_history: stack_history::StackHistory::new(),
        };
        #[cfg(feature = "task-stack-history")]
        system_data
            .stack_history
            .register_task(locals!().stack, CPU_CORE_TASK_NAME);

        self.data.lock_uninit().write(system_data);
        self.current_task.store(current_task, Ordering::Release);

        self.is_init.store(true, Ordering::Release);
    }

    /// Stop the task system on panic
    ///
    /// # Safety
    ///
    /// This should only be called on panic
    pub unsafe fn panic_stop(&self) {
        // Safety: we are panicing
        unsafe {
            LockCellInternal::<Apic>::shatter_permanent(&locals!().apic);
        }
        let mut apic = locals!().apic.lock();
        let mut timer = apic.timer();
        timer.stop();

        self.running.store(false, Ordering::SeqCst);
        self.timer_running.store(false, Ordering::SeqCst);

        unsafe {
            // Safety: try to prevent deadlocks during panic shutdown
            LockCellInternal::<SystemData>::shatter_permanent(&self.data);
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
    pub fn current_task(&self, order: Ordering) -> TaskHandle {
        self.current_task.load(order)
    }

    /// Get some basic information about a [Task]
    pub fn get_task_info(&self, handle: TaskHandle) -> Option<TaskInfo> {
        let data = self.data.lock();
        let Some(task) = data.tasks.get(&handle) else {
            return None;
        };

        Some(TaskInfo {
            handle,
            name: task.name,
        })
    }

    /// Enable the task system and starts the timer
    ///
    /// While running the task system can interrupt
    /// the executing task at any time in order to switch to another system.
    ///
    /// returns the interrupt vector that was previously used by the timer
    pub fn start(&self) -> Result<Option<InterruptVector>, TimerError> {
        if self.is_running() {
            warn!("already running");
            return Ok(None);
        }
        info!("Starting task system");

        let _guard = unsafe { InternalGuard::non_interrupt_enter() };

        self.running.store(true, Ordering::Release);

        let res = self.start_timer(self.data.lock().as_ref());
        res
    }

    /// Pauses the task system.
    ///
    /// use [Self::start] to restart it
    pub fn pause(&self) {
        if !self.is_running() {
            warn!("not running. pause is noop");
            return;
        }

        debug!("stopping timer");
        let _guard = unsafe { InternalGuard::non_interrupt_enter() };

        self.running.store(false, Ordering::Release);
        self.stop_timer(self.data.lock().as_ref());
    }

    /// Start/restart the APIC timer to switch between tasks
    ///
    /// returns the interrupt vector that was previously used by the timer
    fn start_timer(&self, _data_guard: &SystemData) -> Result<Option<InterruptVector>, TimerError> {
        let mut apic = locals!().apic.lock();
        let mut timer = apic.timer();

        if timer.is_running() {
            assert!(self.timer_running.load(Ordering::Acquire));
            error!("Can't start TaskSystem timer if timer is already running");
            return Err(TimerError::TimerRunning);
        }
        assert!(!self.timer_running.load(Ordering::Acquire));
        // no need to set a handler since, ContextSwitch uses a special case handler that
        // can't be changed
        let mut old_vec = timer.enable_interrupt_hander(InterruptVector::ContextSwitch)?;
        if let Some(int_vec) = old_vec
            && int_vec == InterruptVector::ContextSwitch
        {
            old_vec = None;
        }

        let timer_cycles = u32::try_from(timer.rate_mhz() * 1_000_000 / 1_000)
            .expect("timer duration should really fit into a u32")
            * 100; // TODO temp 

        let config = TimerConfig {
            divider: TimerDivider::DivBy1,
            duration: timer_cycles, // TODO what is a good interval
        };

        self.timer_running.store(true, Ordering::Release);
        timer.start(TimerMode::Periodic(config))?;
        debug!("timer started");

        Ok(old_vec)
    }

    /// Stop/pause the APIC timer
    fn stop_timer(&self, _data_guard: &SystemData) {
        let mut apic = locals!().apic.lock();
        let mut timer = apic.timer();
        assert_eq!(timer.vector(), Some(InterruptVector::ContextSwitch));
        self.timer_running.store(false, Ordering::Release);

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
    ) -> Result<TaskHandle, MemError>
    where
        F: FnOnce() -> (),
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
        registers.rsi = unsafe { core::mem::transmute(task_meta) };

        // TODO locals is wrong here. I need the gdt for the cpu core that the task is launched on
        let segments = locals!().gdt.kernel_segments();

        let launch_interrupt_fake = TaskInterrupt {
            stack_frame: InterruptStackFrame::new(
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
            name: task_definition.debug_name,
            stack: Some(stack),
            last_interrupt: Some(launch_interrupt_fake),
        };

        let mut data = self.data.lock();
        let _guard = unsafe { InternalGuard::non_interrupt_enter() };

        #[cfg(feature = "task-stack-history")]
        data.stack_history
            .register_task(stack.pages, task.name.unwrap_or(""));

        let new_handle = TaskHandle::take_next();
        let old_task_value = data.tasks.insert(new_handle, task);
        assert!(old_task_value.is_none());

        debug!(
            "new task {:?} stack: {:p} - {:p}",
            new_handle,
            stack.start_addr(),
            stack_end_minus_task
        );

        data.interrupted.push_back(new_handle);

        if self.is_running() && !self.timer_running.load(Ordering::Acquire) {
            if let Some(old_vec) = self
                .start_timer(&data)
                .expect("Restarting timer should work.")
            {
                warn!("TaskSystem is overwriting timer interrupt vector. Old vector: {old_vec}");
            }
        }

        Ok(new_handle)
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
        let mut data = this.data.lock();

        let current_handle = this.current_task.load(Ordering::Acquire);
        let current_task = data
            .tasks
            .remove(&current_handle)
            .expect("Current task should always exist");

        assert!(current_task.last_interrupt.is_none());
        warn!(
            "terminate task {current_handle:?} {}",
            current_task.name.unwrap_or("")
        );

        // NOTE can't drop current_task right now, this function uses the stack
        // owned by it. Dropping it now would also unmap the stack. Instead we
        // keep track of terminated tasks and drop them later.
        data.terminated.push(current_task);

        if let Some(next_task_handle) = data.interrupted.pop_front() {
            debug!("next task: {next_task_handle:?}");
            this.resume_task(data, next_task_handle, int_guard);
        } else {
            info!("No tasks ready to resume. Halting");
            if this.timer_running.load(Ordering::Acquire) {
                this.stop_timer(&data);
            }
            drop(data);
            // reenable interrupts befor halt
            drop(int_guard);
            cpu::halt();
        }
    }

    /// Called by the context-switch interrupt handler
    fn task_interrupted(interrupt: TaskInterrupt, int_guard: InternalGuard) -> ! {
        // Called from interrupt. This needs to be the TaskSystem for the current cpu
        let this = &locals!().task_system;

        let mut data = this.data.lock();

        // TODO this is not the best time to drop terminated tasks. Instead I want
        // to have some kind of cleanup task. But this works for now
        // TODO I should be able to change PageTable/PageAlloc/PhysAlloc locks to be preemtable
        // once this is no longer in the context-switch path
        // data.terminated.clear();

        let current_handle = this.current_task.load(Ordering::Acquire);

        debug_assert!(verify_interrupt_frame(
            interrupt.stack_frame,
            &data.tasks[&current_handle],
            &data
        ));

        let current_task = data
            .tasks
            .get_mut(&current_handle)
            .expect("Current task should always exist");

        assert!(current_task.last_interrupt.is_none());
        current_task.last_interrupt = Some(interrupt);

        data.interrupted.push_back(current_handle);

        let next_task_handle = data
            .interrupted
            .pop_front()
            .expect("This should always at least contain the currently interrupted task");

        this.resume_task(data, next_task_handle, int_guard);
    }

    /// resumes a [Task]
    fn resume_task<L: LockCell<SystemData>>(
        &self,
        mut data: LockCellGuard<'_, SystemData, L>,
        next_task_handle: TaskHandle,
        int_guard: InternalGuard,
    ) -> ! {
        let next_task = data
            .tasks
            .get_mut(&next_task_handle)
            .expect("Task should always exist if we find the handle");

        let Some(next_task_interrupt) = next_task.last_interrupt.take() else {
            panic!("next task was never iterrupted");
        };
        self.current_task.store(next_task_handle, Ordering::Release);

        drop(data);

        next_task_interrupt.resume(int_guard)
    }
}

fn verify_interrupt_frame<L: LockCell<SystemData>>(
    stack_frame: InterruptStackFrame,
    task: &Task,
    system_data: &LockCellGuard<'_, SystemData, L>,
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
        stack: {:p}-{:p}",
        task.name.unwrap_or(""),
        stack_ptr,
        stack.start_addr(),
        stack.end_addr()
    );

    #[cfg(feature = "task-stack-history")]
    if let Some((stack, owner)) = system_data.stack_history.find(stack_ptr) {
        error!(
            "It looks like the stack pointer, points into the stack of another task
            interrupted task: {}
            stack owner: {}
            stack pointer: {:p}
            stack: {:p}-{:p}",
            task.name.unwrap_or(""),
            owner,
            stack_ptr,
            stack.start_addr(),
            stack.end_addr(),
        );
    } else {
        error!(
            "Could not find stack for stack pointer.
            interrupted task: {}
            stack pointer: {:p}",
            task.name.unwrap_or(""),
            stack_ptr
        );
    }

    false
}

/// The state of an interrupted [Task]
///
/// can be used to [Self::resume] a [Task]
struct TaskInterrupt {
    stack_frame: InterruptStackFrame,
    registers: InterruptRegisterState,
    xsave_area: Option<Box<[u8]>>,
}

impl TaskInterrupt {
    /// resumes the interrupted task
    fn resume(self, int_guard: InternalGuard) -> ! {
        // NOTE: logging here causes deadlock
        #[inline]
        extern "C" fn dealloc_xsave_area(ptr: *mut u8, len: usize) {
            unsafe {
                // Safety: ptr and len refer to Box<[u8]> that we own
                let _ = Box::from_raw(core::ptr::slice_from_raw_parts_mut(ptr, len));
            }
        }

        let (xsave_area_size, xsave_area) = if let Some(xsave_area) = self.xsave_area {
            (xsave_area.len(), Box::leak(xsave_area).as_mut_ptr())
        } else {
            (0, core::ptr::null_mut())
        };
        assert!(xsave_area.is_aligned_to(64));

        // ensure to keep track of interrupt counters
        // drop manually as the assembly exits the function without automatically dorpping
        drop(int_guard);

        unsafe {
            // Saftey: this has to be done in assembly
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

                eoi = sym Apic::eoi,

                registers = in(reg) &self.registers as *const _,

                rflags = in(reg) self.stack_frame.cpu_flags.bits(),
                new_instruction_pointer = in(reg) self.stack_frame.instruction_pointer.as_u64(),
                new_stack_pointer = in(reg) self.stack_frame.stack_pointer.as_u64(),
                code_segment = in(reg) self.stack_frame.code_segment.0,
                stack_segment = in(reg) self.stack_frame.stack_segment.0,

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
    _int_disable_guard: Option<InterruptDisableGuard>,
}

impl InternalGuard {
    /// Interrupt leading to context switch entered TaskSystem
    ///
    /// Safety:
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
            _int_disable_guard: None,
        }
    }

    /// Enter a context switch without an existing interrupt
    ///
    /// Safety:
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
            _int_disable_guard: Some(disable),
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

/// The context-switch interrupt handler used by the [TaskSystem]
/// [Timer](crate::cpu::apic::timer::Timer).
#[unsafe(naked)]
pub extern "x86-interrupt" fn context_switch_handler(_int_stack_frame: InterruptStackFrame) {
    #[inline]
    extern "C" fn rust_handler(
        stack_frame: &InterruptStackFrame,
        registers: &InterruptRegisterState,
        xsave_area: *mut u8,
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
        let interrupt = TaskInterrupt {
            stack_frame: *stack_frame,
            registers: registers.clone(),
            xsave_area: Some(xsave_area),
        };
        TaskSystem::task_interrupted(interrupt, int_guard);
    }

    #[inline]
    extern "C" fn alloc_xsave_area() -> *const u8 {
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
        alloc_xsave = sym alloc_xsave_area,
        rust_handler = sym rust_handler,
        stack_frame_offset = const size_of::<InterruptRegisterState>(),
        registers_offset = const 0x0,
    );
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

type DynTask = dyn FnOnce() -> ();

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
