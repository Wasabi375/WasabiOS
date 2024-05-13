; Trampoline for APs
ORG 0x0000
SECTION .text
USE16

nop
ALIGN 32, nop
entry:
    cld
    cli

    ; Magic sequence so that we can overwrite cx with a segment value
    ; that actually describes our code position in memory
    nop
    nop
    mov cx, 0xabcd
    nop
    nop

    ; Calculate data and code segment for real mode
    mov ds, cx
    
    ; calculate trampoline base addr based on ds and store in edx
    mov edx, ecx
    shl edx, 4

    ; zero out remaining segments
    xor ax, ax
    mov fs, ax
    mov gs, ax

    ; Set the stack to an invalid address for now. We'll figure out our stack later.
    xor ax, ax
    mov sp, ax


    ; inc test counter TODO temp
    LOCK inc dword [trampoline.test_count]

    
    ; enable FPU

    mov eax, cr0
    or al, 00100010b ; Set numeric error (5) monitor co-processor (1)
    mov cr0, eax

    ; Initialize the FPU
    fninit
    
    ; 3: Debugging Extensions
    mov eax, cr4
    or eax,  1 << 3
    mov cr4, eax

    mov ax, gdt_protected.code
    mov [ds:protected_mode_ap_far_jmp + 0], ax
    mov eax, edx
    add eax, protected_mode_ap
    mov [ds:protected_mode_ap_far_jmp + 2], eax

    mov ebx, eax

    lgdt [ds:gdtr_protected]

    ; Enter protected mode, without paging. 
    ; only enable paging when we enter long mode
    ; 0: Protected Mode
    mov eax, cr0
    or eax, 1 
    mov cr0, eax

    ; Far jump into protected mode
    ; jmp far gdt_protected.code:protected_mode_ap


ALIGN 8, nop
USE32
protected_mode_ap:
    hlt
    ; TODO load page table 
    ; cr3 holds pointer to PML4
;    mov eax, [trampoline.page_table]
;    mov cr3, eax

    ; TODO setup cr4 for long mode
   ; bit 5 Physical Address Extension
    ; bit 7 global pages

    ; Enter protected mode, without paging. 
    ; only enable paging when we enter long mode
    ; 31: Enable paging
    ; 16: Enable write protection for ring 0
    ; 5: Numeric error
    ; 4: Extension type
    ; 1: Monitor co-processor
    ; 0: Protected Mode
;    mov eax, cr0
;    or eax, 1 << 31 | 1 << 16 | 1 << 5 | 1 << 4 | 1 << 1 | 1 << 0
;    mov cr0, eax


    ; lgdt [gdtr]

; TODO enbale long mode
    ; enable long mode
;    mov ecx, 0xC0000080               ; Read from the EFER MSR.
;    rdmsr
;    ; enable long mode (bit 8), and NX (bit 11)
;    ; We should really check if we can also enable bit 14 here (FFXSR)
;    ; but for now, just skip it.
;    or eax,  1 << 8 | 1 << 11
;    wrmsr
;    xor ecx, ecx



;    jmp gdt.code:long_mode_ap
;
;ALIGN 8, nop
;
;USE64
;long_mode_ap:
;    mov rax, gdt.data
;    mov ds, rax
;    mov es, rax
;    mov ss, rax
;
;    xor rcx, rcx
;
;    ;; load the addr of the stack end ptr into rdi
;    mov rdi, [trampoline.stack_end_ptr]
;    
;    ;; load the stack end into rax
;    ;; loop until stack end is not 0
;wait_for_stack
;    mov rax, [rdi]
;    cmp rax, rcx
;    je wait_for_stack
;
;    lock cmpxchg [rdi], rcx
;    jnz wait_for_stack
;
;    ;; load stack end into rsp 
;    ;; we have taken ownership 
;    mov rsp, rax
;
;    mov rbx, [trampoline.ap_entry]
;    jmp [rbx]
;
halt_loop:
    cli
    hlt
    jmp short halt_loop;
    hlt
    hlt

ALIGN 64, nop
SECTION .data

; temporary GDT for protected mode
; see https://wiki.osdev.org/Global_descriptor_table
gdt_protected:
.null dq 0
.code equ $ - gdt_protected
    dw 0xffff       ; limit
    dw 0x0          ; base
    db 0x0          ; base
    db 0b10011010   ; access bits: present + code segment + excecut + readable + accessed
    db 0b11001111   ; 4bit flags: 4KiB limit + protected - 4bit limit
    db 0x0          ; base
.data equ $ - gdt_protected
    dw 0xffff       ; limit
    dw 0x0          ; base
    db 0x0          ; base
    db 0b10010010   ; access bits: present + code segment + excecut + readable + accessed
    db 0b11001111   ; 4bit flags: 4KiB limit + protected - 4bit limit
    db 0x0          ; base
.end equ $ - gdt_protected
ALIGN 4, db 0
gdtr_protected:
    dw gdt_protected.end - 1     ; size
    .offset dd gdt_protected ; offset

;temporary GDT, for long mode
gdt:
.null equ $ - gdt
    dq 0
.code equ $ - gdt
    ; 53: Long mode
    ; 47: Present
    ; 44: Code/data segment
    ; 43: Executable
    ; 41: Readable code segment
    dq 0x00209A0000000000             ; 64-bit code descriptor (exec/read).
.data equ $ - gdt
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

;SECTION .bss

ALIGN 4, db 0
protected_mode_ap_far_jmp:
    times 8 db 0xab

tiny_stack: times 128 db 0
    .end: db 0

SECTION .data

ALIGN 8, nop
trampoline:
    .test_count: dq 0x0;
    .page_table: dq 0xFFFFFFFFFFFFFFFF ; -3
    .stack_end_ptr: dq 0xFFFFFFFFFFFFFFFF ; -2
    .ap_entry: dq 0xFFFFFFFFFFFFFFFF ; -1
    .base: dq 0xFFFFFFFFFFFFFFFF ; 0
