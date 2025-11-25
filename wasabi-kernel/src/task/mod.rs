#![allow(missing_docs)] // FIXME
#![expect(unused_variables, dead_code)] // TODO remove

use core::{
    arch::{asm, naked_asm},
    ptr::DynMetadata,
};

use alloc::{boxed::Box, collections::vec_deque::VecDeque, vec::Vec};
use hashbrown::HashMap;
use log::{debug, error, info, trace, warn};
use shared::{
    alloc_ext::{alloc_buffer_aligned, leak_allocator::LeakAllocator},
    dbg,
    sync::lockcell::{LockCell, LockCellGuard},
};
use x86_64::{
    VirtAddr,
    registers::{
        control::{Cr4, Cr4Flags},
        rflags::{self, RFlags},
        xcontrol::{XCr0, XCr0Flags},
    },
    structures::{
        gdt::SegmentSelector,
        idt::InterruptStackFrame,
        paging::{Size4KiB, Translate},
    },
};

use crate::{
    DEFAULT_STACK_SIZE,
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
    mem::{MemError, page_allocator::PageAllocator, page_table::PageTable, structs::GuardedPages},
    pages_required_for,
    prelude::UnwrapTicketLock,
};

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
        error!("dropping task");
        if let Some(stack) = self.stack.take() {
            unsafe {
                // Safety: Stack is no longer in use
                match stack.unmap() {
                    Ok(unmapped) => {
                        PageAllocator::get_for_kernel()
                            .lock()
                            // pages just unmapped
                            .free_guarded_pages(stack);
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

#[derive(Debug, Clone)]
pub struct TaskDefinition<F> {
    pub task: F,

    pub debug_name: Option<&'static str>,

    pub stack_size: u64,
    pub stack_head_guard: bool,
    pub stack_tail_guard: bool,
}

impl<F> TaskDefinition<F> {
    pub const fn new(task: F) -> Self {
        Self {
            task,
            debug_name: None,
            stack_size: DEFAULT_STACK_SIZE,
            stack_head_guard: true,
            stack_tail_guard: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskHandle(u64);

impl TaskHandle {
    pub const CORE_TASK: Self = TaskHandle(0);
    const FIRST_NON_RESERVED: Self = TaskHandle(1);
}

pub struct TaskSystem {
    tasks: HashMap<TaskHandle, Task>,

    current_task: TaskHandle,

    next_free_handle: TaskHandle,

    interrupted: VecDeque<TaskHandle>,

    terminated: Vec<Task>,
}

pub struct TaskInterrupt {
    stack_frame: InterruptStackFrame,
    registers: InterruptRegisterState,
    xsave_area: Option<Box<[u8]>>,
}

impl TaskInterrupt {
    fn resume(self) -> ! {
        #[inline]
        extern "C" fn dealloc_xsave_area(ptr: *mut u8, len: usize) {
            unsafe {
                // Safety: ptr and len refer to Box<[u8]> that we own
                let _ = Box::from_raw(core::ptr::slice_from_raw_parts_mut(ptr, len));
            }
        }
        warn!("resume at {:p}", self.stack_frame.instruction_pointer);
        let (xsave_area_size, xsave_area) = if let Some(xsave_area) = self.xsave_area {
            (xsave_area.len(), Box::leak(xsave_area).as_mut_ptr())
        } else {
            (0, core::ptr::null_mut())
        };
        assert!(xsave_area.is_aligned_to(64));
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

impl TaskSystem {
    pub unsafe fn init() {
        setup_xsave();

        let mut tasks = HashMap::new();
        tasks.insert(
            TaskHandle::CORE_TASK,
            Task {
                name: Some("cpu-core"),
                last_interrupt: None,
                stack: None,
            },
        );

        let system = TaskSystem {
            tasks,
            current_task: TaskHandle::CORE_TASK,
            next_free_handle: TaskHandle::FIRST_NON_RESERVED,
            interrupted: VecDeque::new(),
            terminated: Vec::new(),
        };
        locals!().task_system.lock_uninit().write(system);
    }

    /// Launch the defined task
    ///
    /// # Safety
    ///
    /// the task may not terminate while references into the tasks stack exist
    pub unsafe fn launch_task<F>(task_definition: TaskDefinition<F>) -> Result<TaskHandle, MemError>
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

        debug!(
            "new task stack: {:p} - {:p}",
            stack.start_addr(),
            stack_end_minus_task
        );

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
        registers.rsi = unsafe {
            // Safety: DynMetadata is a NonNull wrapper and therefor can be transmuted into a u64
            core::mem::transmute(task_meta)
        };
        warn!(
            "prep launch task with rdi: {:#x}, rsi: {:#x}",
            registers.rdi, registers.rsi
        );

        {
            let page_table = PageTable::get_for_kernel().lock();
            dbg!(page_table.translate(VirtAddr::new(0x30000158608)));
        }

        let segments = locals!().gdt.kernel_segments();

        let launch_interrupt_fake = TaskInterrupt {
            stack_frame: InterruptStackFrame::new(
                VirtAddr::new(task_entry as usize as u64),
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

        let mut system = locals!().task_system.lock();
        let new_handle = system.take_next_handle();
        let old_task_value = system.tasks.insert(new_handle, task);
        assert!(old_task_value.is_none());
        system.interrupted.push_back(new_handle);

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
        let mut system = locals!().task_system.lock();

        let current_handle = system.current_task;
        let current_task = system
            .tasks
            .remove(&current_handle)
            .expect("Current task should always exist");

        assert!(current_task.last_interrupt.is_none());
        warn!("terminate task {current_handle:?}");

        // NOTE can't drop current_task right now, this function uses the stack
        // owned by it. Dropping it now would also unmap the stack. Instead we
        // keep track of terminated tasks and drop them later.
        system.terminated.push(current_task);

        if let Some(next_task_handle) = system.interrupted.pop_front() {
            Self::resume_task(system, next_task_handle);
        } else {
            info!("Last task terminated");
            // TODO stop timer
            drop(system);
            cpu::halt();
        }
    }

    pub fn task_interrupted(interrupt: TaskInterrupt) -> ! {
        let mut system = locals!().task_system.lock();

        // TODO this is not the best time to drop terminated tasks. Instead I want
        // to have some kind of cleanup task. But this works for now
        system.terminated.clear();

        let current_handle = system.current_task;
        let current_task = system
            .tasks
            .get_mut(&current_handle)
            .expect("Current task should always exist");

        assert!(current_task.last_interrupt.is_none());
        current_task.last_interrupt = Some(interrupt);

        system.interrupted.push_back(current_handle);

        let next_task_handle = system
            .interrupted
            .pop_front()
            .expect("This should always at least contain the currently interrupted task");

        Self::resume_task(system, next_task_handle);
    }

    fn take_next_handle(&mut self) -> TaskHandle {
        let handle = self.next_free_handle;
        self.next_free_handle = TaskHandle(handle.0 + 1);
        handle
    }

    fn resume_task(
        mut system: LockCellGuard<'_, TaskSystem, UnwrapTicketLock<TaskSystem>>,
        next_task_handle: TaskHandle,
    ) -> ! {
        trace!("resume task {next_task_handle:?}");
        let next_task = system
            .tasks
            .get_mut(&next_task_handle)
            .expect("Task should always exist if we find the handle");

        let Some(next_task_interrupt) = next_task.last_interrupt.take() else {
            panic!("next task was never iterrupted");
        };
        system.current_task = next_task_handle;
        drop(system);

        dbg!(&next_task_interrupt.registers);
        dbg!(&next_task_interrupt.stack_frame);

        next_task_interrupt.resume()
    }
}

/// the number of pages used for context swtich interrupts
///
/// Do I need this stack? I am not really sure
pub const CONTEXT_SWITCH_STACK_PAGE_COUNT: u64 = 8;

#[unsafe(naked)]
pub extern "x86-interrupt" fn context_switch_handler(_int_stack_frame: InterruptStackFrame) {
    #[inline]
    extern "C" fn rust_handler(
        stack_frame: &InterruptStackFrame,
        registers: &InterruptRegisterState,
        xsave_area: *mut u8,
    ) -> ! {
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
        TaskSystem::task_interrupted(interrupt);
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
pub struct InterruptRegisterState {
    pub rax: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,

    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
}

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
}

type DynTask = dyn FnOnce() -> ();

// unsafe fn task_entry(task: Box<dyn FnOnce() -> ()>) -> ! {
unsafe fn task_entry(task_ptr: *mut (), task_dyn: DynMetadata<DynTask>) -> ! {
    warn!(
        "Task launched with args: 0 = {:#x}, 1 = {:#x}",
        task_ptr as u64,
        unsafe { core::mem::transmute::<_, u64>(task_dyn) }
    );

    let task_ptr: *mut DynTask = core::ptr::from_raw_parts_mut(task_ptr, task_dyn);
    let task = unsafe {
        // Safety: task_ptr is valid and LeakAllocator can be used for any allocation
        Box::from_raw_in(task_ptr, LeakAllocator::get())
    };
    task();
    {
        core::hint::black_box(
            foo::test_abi as extern "x86-interrupt" fn(InterruptStackFrame) -> (),
        );
    }

    unsafe {
        // TODO
        // Safety: this is not save. Task might leak stack data
        TaskSystem::terminate_task();
    }
}

mod foo {
    use core::hint::black_box;

    use x86_64::structures::idt::InterruptStackFrame;

    pub extern "x86-interrupt" fn test_abi(isf: InterruptStackFrame) {
        test_pass_by_value(isf);
    }

    #[inline(never)]
    pub extern "C" fn test_pass_by_value(isf: InterruptStackFrame) {
        black_box(isf);
    }
}
