[org 0x7c00]
    ;; setup stack
    mov bp, 0x8000
    mov sp, bp

    mov [BOOT_DRIVE], dl        ; store boot drive in case we override it later

    mov bx, MSG_REAL_MODE
    call print
    call print_nl


    mov bx, 0x9000              ; es: 0x0 bx:0x9000 for loading from disk
    mov dh, 2                   ; read 2 sectors, the bios sets dl for us so we don't touch that

    call disk_load

    call switch_to_pm
    jmp $                       ; this should never be executed


[bits 16]
switch_to_pm:
    cli                         ; clear interupt flag / disable interupts
    lgdt [gdt_descriptor]
    mov eax, cr0
    or eax, 0x1                 ; set 32-bit mode flag in cr0
    mov cr0, eax
    jmp CODE_SEG:init_pm

[bits 32]
init_pm:
    mov ax, DATA_SEG            ; update segment registers
    mov ds, ax
    mov ss, ax
    mov es, ax
    mov fs, ax
    mov gs, ax

    mov ebp, 0x90000            ; update the stack right at the top of the free space
    mov esp, ebp

    call BEGIN_PM

[bits 32]
BEGIN_PM:
    mov ebx, MSG_PROT_MODE
    call print_string_pm
    jmp $


%include "asm/boot-sect-print.asm"       ; provides print expecting string addr at bx
%include "asm/boot-sect-print-hex.asm"   ; provides print_hex expecting WORD(2bytes) at dx
%include "asm/boot-sect-disk.asm"        ; provides disk_load loading 'dh' sectors
                                         ; from drive 'dl' in to ES:BX
%include "asm/gdt-32bit.asm"             ; provides gdt_descriptor
%include "asm/boot-sect-print-32bit.asm" ; provides print_string_pm expecting string addr at ebx


BOOT_DRIVE:
    db 0
MSG_PROT_MODE:
    db 'Entered protected mode (32bit)', 0
MSG_REAL_MODE:
    db 'Starting in real mode (16bit)', 0


;; padding
times 510 - ($-$$) db 0

;; magic number
dw 0xaa55

; boot sector = sector 1 of cyl 0 of head 0 of hdd 0
; from now on = sector 2 ...
times 256 dw 0xdada ; sector 2 = 512 bytes
times 256 dw 0xface ; sector 3 = 512 bytes
