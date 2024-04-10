; Trampoline for APs
ORG 0x0000
SECTION .text
USE16

nop
ALIGN 32, nop
entry:
    cld
    cli
    ; zero our code segment, the assembler thinks we're in CS 0, so make it so.
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax
    ; Load IDT
    lidt [idt]

    ; Set the stack to an invalid address for now. We'll figure out our stack later.
    mov ax, 0
    mov sp, ax
    
    ; cr3 holds pointer to PML4
    mov eax, [trampoline.page_table]
    mov cr3, eax

    ; enable FPU

    mov eax, cr0
    ; define the bits in CR0 we want to clear
    ; cache disable (30), Not-write through (29) Task switched (3), x87 emulation (2)
    and ebx, 1 << 30 | 1 << 29 | 1 << 3 | 1 << 2
    xor ebx, 0xFFFFFFFF ; invert bits to get our mask
    and eax, ebx
    or al, 00100010b ; Set numeric error (5) monitor co-processor (1)
    mov cr0, eax

    ; Initialize the FPU
    fninit
    
    ; 9: FXSAVE/FXRSTOR
    ; 7: Page Global
    ; 5: Page Address Extension
    ; 4: Page Size Extension
    ; or eax, 1 << 9 | 1 << 7 | 1 << 5 | 1 << 4
    ; 5: Page Address Extension
    ; 4: Page Size Extension
    ; or eax, 1 << 5 | 1 << 4
    ; 10: Unmasked SIMD Exceptions
    ; 9: FXSAVE/FXRSTOR
    ; 6: Machine Check Exception	
    ; 5: Page Address Extension
    ; 3: Debugging Extensions
    mov eax, cr4
    or eax, 1 << 10 | 1 << 9 | 1 << 7 | 1 << 6  | 1 << 5 | 1 << 3
    mov cr4, eax

    ; enable long mode
    mov ecx, 0xC0000080               ; Read from the EFER MSR.
    rdmsr
    ; enable long mode (bit 8), and NX (bit 11)
    ; We should really check if we can also enable bit 14 here (FFXSR)
    ; but for now, just skip it.
    or eax,  1 << 8 | 1 << 11
    wrmsr
    xor ecx, ecx


    ; enabling paging and protection simultaneously, along with long mode
    ; causes us to go directly from 16 bit real mode, to 64 bit long mode.
    ; IE: Entirely skip protected mode.
    ; 31: Paging
    ; 16: Enable write protection for ring 0
    ; 5: Numeric error
    ; 4: Extension type
    ; 1: Monitor co-processor
    ; 0: Protected Mode
    mov eax, cr0
    or eax, 1 << 31 | 1 << 16 | 1 << 5 | 1 << 4 | 1 << 1 | 1 << 0
    mov cr0, eax
    lgdt [gdtr]
    jmp gdt.kernel_code:long_mode_ap

ALIGN 8, nop

USE64
long_mode_ap:
    mov rax, gdt.kernel_data
    mov ds, rax
    mov es, rax
    mov ss, rax

    xor rcx, rcx

    mov rax, [trampoline.stack_end]
    
    mov rsp, rax
    mov rbx, [trampoline.code]
    jmp [trampoline.code]
halt_loop:
    cli
    hlt
    jmp short halt_loop;

;temporary GDT, we'll set this in code later.
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
    .offset dq gdt  ; offset
; temporary IDT, we'll also set this in code later.
ALIGN 4, db 0
idt:
    dw 0
    dd 0
tiny_stack: times 128 db 0
    .end: db 0
ALIGN 8, nop
trampoline:
    .page_table: dq 0xFFFFFFFFFFFFFFFF ; -3
    .stack_end: dq 0xFFFFFFFFFFFFFFFF ; -2
    .code: dq 0xFFFFFFFFFFFFFFFF ; -1
    .base: dq 0xFFFFFFFFFFFFFFFF ; 0
