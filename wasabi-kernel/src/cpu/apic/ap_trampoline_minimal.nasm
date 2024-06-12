; Trampoline for APs
ORG 0x8000
USE16
SECTION .text

startup_ap:
    cli
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax
    
    ; Set the stack to an invalid address for now. We'll figure out our stack later.
    mov sp, 0

    ; cr3 holds pointer to PML4
    mov edi, [trampoline.page_table]
    mov cr3, edi


    ; enable FPU
    mov eax, cr0
    and al, 11110011b ; Clear task switched (3) and emulation (2)
    or al, 00100010b ; Set numeric error (5) monitor co-processor (1)
    mov cr0, eax    ; Initialize the FPU

    ; 9: FXSAVE/FXRSTOR
    ; 7: Page Global
    ; 5: Page Address Extension
    ; 4: Page Size Extension
    mov eax, cr4
    or eax, 1 << 9 | 1 << 7 | 1 << 5 | 1 << 4
    mov cr4, eax

    ; init floating point registers
    fninit

    ; load protected mode GDT
    lgdt [gdtr]
 
    ; mask to disable some cr0 bits
    mov eax, 1 << 30 | 1 << 29
    not eax

    ; enable long mode
    mov ecx, 0xC0000080               ; Read from the EFER MSR.
    rdmsr
    or eax,  1 << 8 | 1 << 11         ; Set the Long-Mode-Enable and NXE bit.
    wrmsr

    ; enabling paging and protection simultaneously
    mov ebx, cr0
    and ebx, eax
    ; 31: Paging
    ; 16: write protect kernel
    ; 0: Protected Mode
    or ebx, 1 << 31  | 1
    mov cr0, ebx    ;; This instruction here cause a tripple fault
    
    ; far jump to enable Long Mode and load CS with 64 bit segment
    jmp gdt.kernel_code:long_mode_ap

USE64
long_mode_ap:

    LOCK inc dword [trampoline.test_counter]

    xor rcx, rcx

    ;; load the addr of the stack end ptr into rdi
    mov rdi, [trampoline.stack_end_ptr]
    
    ;; load the stack end into rax
    ;; loop until stack end is not 0
wait_for_stack
    mov rax, [rdi]
    cmp rax, rcx
    je wait_for_stack

    ; cmp rax with [rdi], if equal load rcx into [rdi] and set ZF
    ; otherwise load [rdi] into rax
    lock cmpxchg [rdi], rcx
    ; we want to load the stack ptr (not 0) into rax so we repeat
    ; if [rdi] equals rcx(0)
    jne wait_for_stack

    ;; load stack end into rsp 
    ;; we have taken ownership 
    mov rsp, rax
    
    LOCK inc dword [trampoline.test_counter]
    ;jmp halt_loop

    mov rbx, [trampoline.ap_entry]
    jmp [trampoline.ap_entry]

halt_loop:
    cli
    hlt
    jmp short halt_loop;
    hlt
    hlt



SECTION .data

gdt:
.null equ $ - gdt
    dq 0
.kernel_code equ $ - gdt
    ; 53: Long mode
    ; 47: Present
    ; 44: Code/data segment
    ; 43: Executable
    ; 41: Readable code segment
    dq 0x00209A0000000000             ; 64-bit code descriptor (exec/read).
.kernel_data equ $ - gdt
    ; 47: Present
    ; 44: Code/data segment
    ; 41: Writable data segment
    dq 0x0000920000000000             ; 64-bit data descriptor (read/write).

.end equ $ - gdt
ALIGN 4, db 0
gdtr:
    dw gdt.end - 1 ; size
    dq gdt  ; offset


ALIGN 8, nop
trampoline:
    .test_counter: dq 0x0; -4
    .page_table: dq 0xFFFFFFFFFFFFFFFF ; -3
    .stack_end_ptr: dq 0xFFFFFFFFFFFFFFFF ; -2
    .ap_entry: dq 0xFFFFFFFFFFFFFFFF ; -1
    .base: dq 0xFFFFFFFFFFFFFFFF ; 0
