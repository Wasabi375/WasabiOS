use super::{StaticString, StringError};
use core::slice::from_raw_parts;
use core::str::from_utf8_unchecked;

/// Unsafely marks a branch as unreachable.
#[inline(always)]
#[allow(unused_variables)]
pub(crate) unsafe fn never(s: &str) -> ! {
    #[cfg(debug_assertions)]
    core::panic!("{}", s);
}

// UTF-8 ranges and tags for encoding characters.
const TAG_CONT: u8 = 0b1000_0000;
const TAG_TWO_B: u8 = 0b1100_0000;
const TAG_THREE_B: u8 = 0b1110_0000;
const TAG_FOUR_B: u8 = 0b1111_0000;
const MAX_ONE_B: u32 = 0x80;
const MAX_TWO_B: u32 = 0x800;
const MAX_THREE_B: u32 = 0x10000;

/// Encodes `character` into `string` at the specified position.
#[inline(always)]
pub(crate) unsafe fn encode_char_utf8_unchecked<const N: usize>(
    string: &mut StaticString<N>,
    character: char,
    index: usize,
) {
    let dest = string.vec.mut_ptr_at_unchecked(index);
    let code = character as u32;
    if code < MAX_ONE_B {
        dest.write(code as u8);
        string.vec.set_len(string.len() + 1);
    } else if code < MAX_TWO_B {
        dest.write((code >> 6 & 0x1F) as u8 | TAG_TWO_B);
        dest.offset(1).write((code & 0x3F) as u8 | TAG_CONT);
        string.vec.set_len(string.len() + 2);
    } else if code < MAX_THREE_B {
        dest.write((code >> 12 & 0x0F) as u8 | TAG_THREE_B);
        dest.offset(1).write((code >> 6 & 0x3F) as u8 | TAG_CONT);
        dest.offset(2).write((code & 0x3F) as u8 | TAG_CONT);
        string.vec.set_len(string.len() + 3);
    } else {
        dest.write((code >> 18 & 0x07) as u8 | TAG_FOUR_B);
        dest.offset(1).write((code >> 12 & 0x3F) as u8 | TAG_CONT);
        dest.offset(2).write((code >> 6 & 0x3F) as u8 | TAG_CONT);
        dest.offset(3).write((code & 0x3F) as u8 | TAG_CONT);
        string.vec.set_len(string.len() + 4);
    }
}

/// Fast `const` compatible version of `encode_utf8` for chars with a known length greater than one.
#[inline(always)]
pub(crate) fn encode_utf8_raw(code: u32, len: usize) -> [u8; 4] {
    let mut res = [0u8; 4];
    match (len, &mut res) {
        (2, [a, b, ..]) => {
            *a = (code >> 6 & 0x1F) as u8 | TAG_TWO_B;
            *b = (code & 0x3F) as u8 | TAG_CONT;
        }
        (3, [a, b, c, ..]) => {
            *a = (code >> 12 & 0x0F) as u8 | TAG_THREE_B;
            *b = (code >> 6 & 0x3F) as u8 | TAG_CONT;
            *c = (code & 0x3F) as u8 | TAG_CONT;
        }
        (4, [a, b, c, d]) => {
            *a = (code >> 18 & 0x07) as u8 | TAG_FOUR_B;
            *b = (code >> 12 & 0x3F) as u8 | TAG_CONT;
            *c = (code >> 6 & 0x3F) as u8 | TAG_CONT;
            *d = (code & 0x3F) as u8 | TAG_CONT;
        }
        _ => panic!("Bug in `encode_utf8_raw!"),
    };
    res
}

macro_rules! shift_right_unchecked {
    ($self_var:expr, $from_var:expr, $to_var:expr) => {
        debug_assert!(
            $from_var as usize <= $to_var as usize
                && $to_var as usize + $self_var.len() - $from_var as usize <= $self_var.capacity()
        );
        debug_assert!($self_var.as_str().is_char_boundary($from_var));
        let mp = $self_var.vec.as_mut_ptr();
        mp.add($from_var)
            .copy_to(mp.add($to_var), $self_var.len() - $from_var);
    };
}

macro_rules! shift_left_unchecked {
    ($self_var:expr, $from_var:expr, $to_var:expr) => {
        debug_assert!(
            $to_var as usize <= $from_var as usize && $from_var as usize <= $self_var.len()
        );
        debug_assert!($self_var.as_str().is_char_boundary($from_var));
        let mp = $self_var.vec.as_mut_ptr();
        mp.add($from_var)
            .copy_to(mp.add($to_var), $self_var.len() - $from_var);
    };
}

/// Returns an error if `size` is greater than `limit`.
#[inline(always)]
pub(crate) fn is_inside_boundary(size: usize, limit: usize) -> Result<(), StringError> {
    match size <= limit {
        false => Err(StringError::OutOfBounds),
        true => Ok(()),
    }
}

/// Returns an error if `index` is not at a valid UTF-8 character boundary.
#[must_use]
#[inline(always)]
pub(crate) fn str_is_char_boundary(s: &str, index: usize) -> bool {
    let len = s.len();
    if index == 0 || index == len {
        true
    } else if index > len {
        false
    } else {
        unsafe { (*s.as_ptr().add(index)) as i8 >= -0x40 }
    }
}

/// Returns an error if `index` is not at a valid UTF-8 character boundary.
#[inline(always)]
pub(crate) fn is_char_boundary<const N: usize>(
    string: &StaticString<N>,
    index: usize,
) -> Result<(), StringError> {
    match str_is_char_boundary(string.as_str(), index) {
        false => Err(StringError::NotCharBoundary),
        true => Ok(()),
    }
}

/// Truncates `slice` to the specified size (ignoring the last few bytes if they form a partial
/// `char`).
#[inline]
pub(crate) fn truncate_str(slice: &str, size: usize) -> &str {
    if str_is_char_boundary(slice, size) {
        unsafe { from_utf8_unchecked(from_raw_parts(slice.as_ptr(), size)) }
    } else if size < slice.len() {
        let mut index = size - 1;
        while !str_is_char_boundary(slice, index) {
            index -= 1;
        }
        unsafe { from_utf8_unchecked(from_raw_parts(slice.as_ptr(), index)) }
    } else {
        slice
    }
}

/// Macro to avoid code duplication in `char`-pushing methods.
macro_rules! push_char_unchecked_internal {
    ($self_var:expr, $char_var:expr, $len:expr) => {
        #[allow(unused_unsafe)]
        match $len {
            1 => unsafe { $self_var.vec.push_unchecked($char_var as u8) },
            _ => {
                let old_length = $self_var.len();
                unsafe {
                    $crate::string::string_utils::encode_utf8_raw($char_var, $len)
                        .as_ptr()
                        .copy_to_nonoverlapping(
                            $self_var.vec.mut_ptr_at_unchecked(old_length),
                            $len,
                        );
                    $self_var.vec.set_len(old_length + $len);
                }
            }
        };
    };
}

/// Macro to avoid code duplication in `&str`-pushing methods.
macro_rules! push_str_unchecked_internal {
    ($self_var:expr, $str_var:expr, $self_len_var:expr, $str_len_var:expr) => {
        #[allow(unused_unsafe)]
        unsafe {
            let dest = $self_var.vec.mut_ptr_at_unchecked($self_len_var);
            $str_var.as_ptr().copy_to_nonoverlapping(dest, $str_len_var);
            $self_var.vec.set_len($self_len_var + $str_len_var);
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::str::from_utf8;

    #[test]
    fn truncate() {
        assert_eq!(truncate_str("i", 10), "i");
        assert_eq!(truncate_str("iiiiii", 3), "iii");
        assert_eq!(truncate_str("🤔🤔🤔", 5), "🤔");
        assert_eq!(truncate_str("🤔🤔🤔", 0), "");
    }

    #[test]
    fn shift_right() {
        let mut ls = StaticString::<20>::try_from_str("abcdefg").unwrap();
        unsafe {
            shift_right_unchecked!(ls, 0usize, 4usize);
        }
        unsafe { ls.vec.set_len(ls.len() + 4) };
        assert_eq!(ls.as_str(), "abcdabcdefg");
    }

    #[test]
    fn shift_left() {
        let mut ls = StaticString::<20>::try_from_str("abcdefg").unwrap();
        unsafe {
            shift_left_unchecked!(ls, 1usize, 0usize);
        }
        unsafe { ls.vec.set_len(ls.len() - 1) };
        assert_eq!(ls.as_str(), "bcdefg");
    }

    #[test]
    fn shift_nop() {
        let mut ls = StaticString::<20>::try_from_str("abcdefg").unwrap();
        unsafe {
            shift_right_unchecked!(ls, 0usize, 0usize);
        }
        assert_eq!(ls.as_str(), "abcdefg");
        unsafe {
            shift_left_unchecked!(ls, 0usize, 0usize);
        }
        assert_eq!(ls.as_str(), "abcdefg");
    }

    #[test]
    fn encode_char_utf8() {
        let mut string = StaticString::<20>::default();
        unsafe {
            encode_char_utf8_unchecked(&mut string, 'a', 0);
            assert_eq!(from_utf8(&string.as_mut_bytes()).unwrap(), "a");
            let mut string = StaticString::<20>::try_from_str("a").unwrap();
            encode_char_utf8_unchecked(&mut string, '🤔', 1);
            assert_eq!(from_utf8(&string.as_mut_bytes()[..5]).unwrap(), "a🤔");
        }
    }
}
