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
    .page_table: dq 0xFFFFFFFFFFFFFFFF ; -3
    .stack_end_ptr: dq 0xFFFFFFFFFFFFFFFF ; -2
    .ap_entry: dq 0xFFFFFFFFFFFFFFFF ; -1
    .base: dq 0xFFFFFFFFFFFFFFFF ; 0
