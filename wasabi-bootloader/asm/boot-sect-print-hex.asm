[bits 16]

    ;; prints the number stored in 4byte value in dx as hex
print_hex:
    pusha
    mov cx, 0

    ;; Strategy: Looking at a single byte we can convert it to hex by first adding '0' (0x30)
    ;; to convert it to '0'-'9'. If it is larger than 0x39 the byte does not fall within 0-9
    ;; so we need to convert it to 'A-F'(0x41 - 0x46) by adding 0x7.
hex_loop:
    cmp cx, 4
    je end

    mov ax, dx                  ; use ax as working register
    and ax, 0x000f              ; we only care about the last byte
    add al, 0x30                ; convert to '0-9' range

    cmp al, 0x39                ; if byte > 9 we need to convert to 'A-F'
    jle step2
    add al, 0x7                 ; add 0x7 to convert to 'A-F'

step2:
    mov bx, HEX_OUT + 5         ; set bx to last char in out string
    sub bx, cx                  ; mov bx back to this bytes position
    mov [bx], al                ; store al in [bx]

    ror dx, 4                   ; a hex char can represent 4 bits
    add cx, 1

    jmp hex_loop

end:
    mov bx, HEX_OUT
    call print

    popa
    ret

HEX_OUT:
    db "0x0000", 0
