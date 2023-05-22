use core::{fmt::Write, str::FromStr};
use staticvec::StaticString;

/// The 8 standard colors.
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
    TrueColor { r: u8, g: u8, b: u8 },
}

#[allow(missing_docs)]
impl Color {
    pub const TO_STR_MAX_LEN: usize = 16;
    pub fn to_fg_str(&self) -> StaticString<{ Self::TO_STR_MAX_LEN }> {
        match *self {
            Color::Black => StaticString::from_str("30"),
            Color::Red => StaticString::from_str("31"),
            Color::Green => StaticString::from_str("32"),
            Color::Yellow => StaticString::from_str("33"),
            Color::Blue => StaticString::from_str("34"),
            Color::Magenta => StaticString::from_str("35"),
            Color::Cyan => StaticString::from_str("36"),
            Color::White => StaticString::from_str("37"),
            Color::BrightBlack => StaticString::from_str("90"),
            Color::BrightRed => StaticString::from_str("91"),
            Color::BrightGreen => StaticString::from_str("92"),
            Color::BrightYellow => StaticString::from_str("93"),
            Color::BrightBlue => StaticString::from_str("94"),
            Color::BrightMagenta => StaticString::from_str("95"),
            Color::BrightCyan => StaticString::from_str("96"),
            Color::BrightWhite => StaticString::from_str("97"),
            Color::TrueColor { r, g, b } => {
                let mut str = StaticString::new();
                write!(&mut str, "38;2;{};{};{}", r, g, b).unwrap();
                str
            }
        }
    }

    pub fn to_bg_str(&self) -> StaticString<{ Self::TO_STR_MAX_LEN }> {
        match *self {
            Color::Black => StaticString::from_str("40"),
            Color::Red => StaticString::from_str("41"),
            Color::Green => StaticString::from_str("42"),
            Color::Yellow => StaticString::from_str("43"),
            Color::Blue => StaticString::from_str("44"),
            Color::Magenta => StaticString::from_str("45"),
            Color::Cyan => StaticString::from_str("46"),
            Color::White => StaticString::from_str("47"),
            Color::BrightBlack => StaticString::from_str("100"),
            Color::BrightRed => StaticString::from_str("101"),
            Color::BrightGreen => StaticString::from_str("102"),
            Color::BrightYellow => StaticString::from_str("103"),
            Color::BrightBlue => StaticString::from_str("104"),
            Color::BrightMagenta => StaticString::from_str("105"),
            Color::BrightCyan => StaticString::from_str("106"),
            Color::BrightWhite => StaticString::from_str("107"),
            Color::TrueColor { r, g, b } => {
                let mut str = StaticString::new();
                write!(&mut str, "48;2;{};{};{}", r, g, b).unwrap();
                str
            }
        }
    }
}

impl<'a> From<&'a str> for Color {
    fn from(src: &str) -> Self {
        src.parse().unwrap_or(Color::White)
    }
}

impl<const N: usize> From<StaticString<N>> for Color {
    fn from(src: StaticString<N>) -> Self {
        src.parse().unwrap_or(Color::White)
    }
}

impl FromStr for Color {
    type Err = ();

    fn from_str(src: &str) -> Result<Self, Self::Err> {
        let mut src = StaticString::<20>::try_from(src).map_err(|_| ())?;
        src.make_ascii_lowercase();

        match src.as_ref() {
            "black" => Ok(Color::Black),
            "red" => Ok(Color::Red),
            "green" => Ok(Color::Green),
            "yellow" => Ok(Color::Yellow),
            "blue" => Ok(Color::Blue),
            "magenta" => Ok(Color::Magenta),
            "purple" => Ok(Color::Magenta),
            "cyan" => Ok(Color::Cyan),
            "white" => Ok(Color::White),
            "bright black" => Ok(Color::BrightBlack),
            "bright red" => Ok(Color::BrightRed),
            "bright green" => Ok(Color::BrightGreen),
            "bright yellow" => Ok(Color::BrightYellow),
            "bright blue" => Ok(Color::BrightBlue),
            "bright magenta" => Ok(Color::BrightMagenta),
            "bright cyan" => Ok(Color::BrightCyan),
            "bright white" => Ok(Color::BrightWhite),
            _ => Err(()),
        }
    }
}

#[cfg(test)]
mod tests {
    pub use super::*;

    mod from_str {
        pub use super::*;

        macro_rules! make_test {
            ( $( $name:ident: $src:expr => $dst:expr),* ) => {

                $(
                    #[test]
                    fn $name() {
                        let color : Color = $src.into();
                        assert_eq!($dst, color)
                    }
                )*
            }
        }

        make_test!(
            black: "black" => Color::Black,
            red: "red" => Color::Red,
            green: "green" => Color::Green,
            yellow: "yellow" => Color::Yellow,
            blue: "blue" => Color::Blue,
            magenta: "magenta" => Color::Magenta,
            purple: "purple" => Color::Magenta,
            cyan: "cyan" => Color::Cyan,
            white: "white" => Color::White,
            brightblack: "bright black" => Color::BrightBlack,
            brightred: "bright red" => Color::BrightRed,
            brightgreen: "bright green" => Color::BrightGreen,
            brightyellow: "bright yellow" => Color::BrightYellow,
            brightblue: "bright blue" => Color::BrightBlue,
            brightmagenta: "bright magenta" => Color::BrightMagenta,
            brightcyan: "bright cyan" => Color::BrightCyan,
            brightwhite: "bright white" => Color::BrightWhite,

            invalid: "invalid" => Color::White,
            capitalized: "BLUE" => Color::Blue,
            mixed_case: "bLuE" => Color::Blue
        );
    }

    mod fromstr {
        pub use super::*;

        #[test]
        fn parse() {
            let color: Result<Color, _> = "blue".parse();
            assert_eq!(Ok(Color::Blue), color)
        }

        #[test]
        fn error() {
            let color: Result<Color, ()> = "bloublou".parse();
            assert_eq!(Err(()), color)
        }
    }
}
