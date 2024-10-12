//! A collection of utility functions

use alloc::string::String;
use core::fmt::Write;
use core::mem::size_of;
use x86_64::VirtAddr;

use crate::serial_print;

const INVISIBLE_CHAR: char = '.';

const HEX_DUMP_LOG_BYTE_WIDTH: usize = 16;
const HEX_DUMP_SERIAL_BYTE_WIDTH: usize = 16;

/// hex dumps the memory at `start` to the logger
///
/// # Safety:
///
/// `start` up to `start + length - 1` must be valid pointers
pub unsafe fn log_hex_dump<M: AsRef<str>>(
    message: M,
    level: log::Level,
    target: &str,
    start: VirtAddr,
    length: usize,
) {
    let line_count = (length / HEX_DUMP_LOG_BYTE_WIDTH) + 1;
    let dump_length = line_count * (line_width(HEX_DUMP_LOG_BYTE_WIDTH) + 1);

    let mut dump = String::with_capacity(dump_length);

    // Safety: see function definition
    dump.extend(unsafe { hex_dump_iter(start, length, HEX_DUMP_LOG_BYTE_WIDTH) });

    log::log!(target: target, level, "{}\n{}", message.as_ref(), dump);
}

/// hex dumps the memory in the buffer to the logger
pub fn log_hex_dump_buf<M: AsRef<str>, T>(
    message: M,
    level: log::Level,
    target: &str,
    buffer: &[T],
) {
    let vaddr = VirtAddr::from_ptr(buffer);
    unsafe {
        // Safety: vaddr points to valid slice
        log_hex_dump(message, level, target, vaddr, buffer.len() * size_of::<T>());
    }
}

/// hex dumps the memory of the data to the logger
pub fn log_hex_dump_struct<M: AsRef<str>, T>(
    message: M,
    level: log::Level,
    target: &str,
    data: &T,
) {
    let vaddr = VirtAddr::from_ptr(data);
    unsafe {
        // Safety: vaddr points to valid reference
        log_hex_dump(message, level, target, vaddr, size_of::<T>());
    }
}

/// hex dumps the memory at `start` to the SERIAL1
///
/// # Safety:
///
/// `start` up to `start + length - 1` must be valid pointers
pub unsafe fn serial_hex_dump<M: AsRef<str>>(start: VirtAddr, length: usize) {
    // Safety: see function definition
    for line in unsafe { hex_dump_iter(start, length, HEX_DUMP_SERIAL_BYTE_WIDTH) } {
        serial_print!("{}", line);
    }
}

/// Safety:
///
/// `start` must be a valid ptr for the next `length` bytes
unsafe fn hex_dump_iter(
    start: VirtAddr,
    length: usize,
    width: usize,
) -> impl Iterator<Item = String> {
    assert!(length > 0);
    let end_inclusive = start + (length - 1) as u64;

    (start..=end_inclusive)
        .step_by(width)
        .map(move |slice_start| {
            (
                slice_start,
                core::cmp::min(slice_start + (width - 1) as u64, end_inclusive),
                slice_start - start,
            )
        })
        .map(|(start, end, offset)| (start, (end - start) + 1, offset))
        .map(|(start, length, offset)| (start.as_ptr::<u8>(), length as usize, offset))
        .map(|(ptr, length, offset)| (core::ptr::slice_from_raw_parts(ptr, length), offset))
        .map(|(ptr, offset)| (unsafe { &*ptr }, offset))
        .map(move |(slice, offset)| slice_to_hex_dump_line(slice, width, offset))
}

fn line_width(byte_count: usize) -> usize {
    // 1 byte = 2 hex + 1 space
    // "| " separator
    // 1 byte = 1 ascii
    // new line
    4 * byte_count + 3
}

fn slice_to_hex_dump_line(bytes: &[u8], width: usize, offset: u64) -> String {
    let mut out = String::with_capacity(line_width(width));

    write!(out, "{:4X} | ", offset).expect("failed to write to output buffer");

    for b in bytes {
        write!(out, "{:02X} ", b).expect("failed to write to output buffer");
    }

    // pad the output to width "bytes"
    for _ in 0..(width - bytes.len()) {
        out.push_str("   ");
    }

    out.push_str("| ");

    for b in bytes {
        let c = *b as char;
        if c.is_ascii_alphanumeric() || c.is_ascii_punctuation() {
            out.push(c)
        } else {
            out.push(INVISIBLE_CHAR);
        }
    }

    out.push('\n');

    out
}
