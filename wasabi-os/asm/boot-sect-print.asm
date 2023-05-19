[bits 16]

    ;; Prints a null-terminated string.
    ;; Expects base address of the string in bx
print:
    pusha                    ; push current registers


    ;; while string[i] != null { print string[i]; i++ }
start:
    ;; while string[i] != null
    mov al, [bx]
    cmp al, 0
    je done

    ;; print string[i]
    mov ah, 0x0e
    int 0x10

    ;; i++
    add bx, 1

    ;; loop end
    jmp start

done:
    popa                        ; restore registers
    ret


print_nl:
    pusha

    mov ah, 0x0e
    mov al, 0x0a ; newline char
    int 0x10
    mov al, 0x0d ; carriage return
    int 0x10

    popa
    ret
