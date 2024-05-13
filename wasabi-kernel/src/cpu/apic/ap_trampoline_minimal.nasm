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

    mov ds, cx
    
    ; calculate trampoline base addr based on ds and store in edx
    mov edx, ecx
    shl edx, 4

    ; Set the stack to an invalid address for now. We'll figure out our stack later.
    xor ax, ax
    mov sp, ax

    ; set code segment for indirect far jump
    mov ax, gdt_protected.code
    mov [ds:protected_mode_ap_far_jmp + 0], ax
    ; set eip for indirect far jump
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
    jmp far dword [protected_mode_ap_far_jmp]


ALIGN 8, nop
USE32
protected_mode_ap:
    hlt

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
    dd gdt_protected ; offset

ALIGN 4, db 0
protected_mode_ap_far_jmp:
    times 6 db 0xab
    times 2 db 0x0

ALIGN 8, nop
trampoline:
    .page_table: dq 0xFFFFFFFFFFFFFFFF ; -3
    .stack_end_ptr: dq 0xFFFFFFFFFFFFFFFF ; -2
    .ap_entry: dq 0xFFFFFFFFFFFFFFFF ; -1
    .base: dq 0xFFFFFFFFFFFFFFFF ; 0
