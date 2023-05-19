#!/bin/sh

die() {
    echo "die: $*"
    exit 1
}

nasm -f bin asm/boot-sect-main.asm -o boot-sect-main.bin || die "nasm failed"


WINDOWS_NATIVE_QEMU="0"

if command -v wslpath >/dev/null; then
    WINDOWS_NATIVE_QEMU="1"
fi

if [ -z "$WASABI_QEMU_BIN" ]; then
    if [ "$WINDOWS_NATIVE_QEMU" -eq "1" ]; then
        PATH=$PATH:/mnt/c/Windows/System32
        QEMU_INSTALL_DIR=$(reg.exe query 'HKLM\Software\QEMU' /v Install_Dir /t REG_SZ | grep '^    Install_Dir' | sed 's/    / /g' | cut -f4- -d' ')
        if [ -z "$QEMU_INSTALL_DIR" ]; then
            if [ "$VIRTUALIZATION_SUPPORT" -eq "0" ]; then
                die "Could not determine where QEMU for Windows is installed. Please make sure QEMU is installed or set SERENITY_QEMU_BIN if it is already installed."
            fi
        else
            QEMU_BINARY_PREFIX="$(wslpath -- "${QEMU_INSTALL_DIR}" | tr -d '\r\n')/"
            QEMU_BINARY_SUFFIX=".exe"
        fi
    else
        echo "auto detect QEMU only implemented for WSL"
        die
    fi

    WASABI_QEMU_BIN="${QEMU_BINARY_PREFIX}qemu-system-x86_64${QEMU_BINARY_SUFFIX}"
fi

if [ -z "$WASABI_DISK_IMAGE" ]; then
    WASABI_DISK_IMAGE="boot-sect-main.bin"
fi


if [ "$WINDOWS_NATIVE_QEMU" -eq "1" ]; then
    WASABI_DISK_IMAGE=$(wslpath -a -w "$WASABI_DISK_IMAGE")
fi

WASABI_BOOT_DRIVE="-drive format=raw,file=${WASABI_DISK_IMAGE}"

"${WASABI_QEMU_BIN}" $WASABI_BOOT_DRIVE
