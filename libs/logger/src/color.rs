use core::fmt::Write;

/// Ansi color
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Color {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    BrightBlack,
    BrightRed,
    BrightGreen,
    BrightYellow,
    BrightBlue,
    BrightMagenta,
    BrightCyan,
    BrightWhite,
    Index(u8),
    TrueColor { r: u8, g: u8, b: u8 },
}

pub const ANSI_RESET: &'static str = "\x1b[0m";

pub(super) fn write_ansi_fg_color<W: Write>(writer: &mut W, color: Color) -> core::fmt::Result {
    match color {
        Color::Black => writer.write_fmt(format_args!("\x1b[30m")),
        Color::Red => writer.write_fmt(format_args!("\x1b[31m")),
        Color::Green => writer.write_fmt(format_args!("\x1b[32m")),
        Color::Yellow => writer.write_fmt(format_args!("\x1b[33m")),
        Color::Blue => writer.write_fmt(format_args!("\x1b[34m")),
        Color::Magenta => writer.write_fmt(format_args!("\x1b[35m")),
        Color::Cyan => writer.write_fmt(format_args!("\x1b[36m")),
        Color::White => writer.write_fmt(format_args!("\x1b[37m")),
        Color::BrightBlack => writer.write_fmt(format_args!("\x1b[90m")),
        Color::BrightRed => writer.write_fmt(format_args!("\x1b[91m")),
        Color::BrightGreen => writer.write_fmt(format_args!("\x1b[92m")),
        Color::BrightYellow => writer.write_fmt(format_args!("\x1b[93m")),
        Color::BrightBlue => writer.write_fmt(format_args!("\x1b[94m")),
        Color::BrightMagenta => writer.write_fmt(format_args!("\x1b[95m")),
        Color::BrightCyan => writer.write_fmt(format_args!("\x1b[96m")),
        Color::BrightWhite => writer.write_fmt(format_args!("\x1b[97m")),
        Color::Index(ref index) => writer.write_fmt(format_args!("\x1b[38;5;{}m", index)),
        Color::TrueColor {
            ref r,
            ref g,
            ref b,
        } => writer.write_fmt(format_args!("\x1b[38;2;{};{};{}m", r, g, b)),
    }
}

#[allow(unused)]
pub(super) fn write_ansi_bg_color<W: Write>(writer: &mut W, color: Color) -> core::fmt::Result {
    match color {
        Color::Black => writer.write_fmt(format_args!("\x1b[40m")),
        Color::Red => writer.write_fmt(format_args!("\x1b[41m")),
        Color::Green => writer.write_fmt(format_args!("\x1b[42m")),
        Color::Yellow => writer.write_fmt(format_args!("\x1b[43m")),
        Color::Blue => writer.write_fmt(format_args!("\x1b[44m")),
        Color::Magenta => writer.write_fmt(format_args!("\x1b[45m")),
        Color::Cyan => writer.write_fmt(format_args!("\x1b[46m")),
        Color::White => writer.write_fmt(format_args!("\x1b[47m")),
        Color::BrightBlack => writer.write_fmt(format_args!("\x1b[100m")),
        Color::BrightRed => writer.write_fmt(format_args!("\x1b[101m")),
        Color::BrightGreen => writer.write_fmt(format_args!("\x1b[102m")),
        Color::BrightYellow => writer.write_fmt(format_args!("\x1b[103m")),
        Color::BrightBlue => writer.write_fmt(format_args!("\x1b[104m")),
        Color::BrightMagenta => writer.write_fmt(format_args!("\x1b[105m")),
        Color::BrightCyan => writer.write_fmt(format_args!("\x1b[106m")),
        Color::BrightWhite => writer.write_fmt(format_args!("\x1b[107m")),
        Color::Index(ref index) => writer.write_fmt(format_args!("\x1b[38;5;{}m", index)),
        Color::TrueColor {
            ref r,
            ref g,
            ref b,
        } => writer.write_fmt(format_args!("\x1b[38;2;{};{};{}m", r, g, b)),
    }
}
