//!Coloring terminal so simple, you already know how to do it !
//!
//!    use colored::Colorize;
//!
//!    "this is blue".blue();
//!    "this is red".red();
//!    "this is red on blue".red().on_blue();
//!    "this is also red on blue".on_blue().red();
//!    "you can use truecolor values too!".truecolor(0, 255, 136);
//!    "background truecolor also works :)".on_truecolor(135, 28, 167);
//!    "you can also make bold comments".bold();
//!    println!("{} {} {}", "or use".cyan(), "any".italic().yellow(), "string type".cyan());
//!    "or change advice. This is red".yellow().blue().red();
//!    "or clear things up. This is default color and style".red().bold().clear();
//!    "purple and magenta are the same".purple().magenta();
//!    "bright colors are also allowed".bright_blue().on_bright_white();
//!    "you can specify color by string".color("blue").on_color("red");
//!    "and so are normal and clear".normal().clear();
//!    String::from("this also works!").green().bold();
//!    format!("{:30}", "format works as expected. This will be padded".blue());
//!    format!("{:.3}", "and this will be green but truncated to 3 chars".green());
//!
//!
//! See [the `Colorize` trait](./trait.Colorize.html) for all the methods.
//!
#![warn(missing_docs)]
// linking tests fails if we add no_main
#![cfg_attr(not(test), no_std)]

#[macro_use]
extern crate lazy_static;

#[cfg(test)]
extern crate rspec;

mod color;
pub mod control;
mod style;

pub use self::customcolors::CustomColor;

/// Custom colors support.
pub mod customcolors;

pub use color::*;
use staticvec::{CapacityError, StaticString};

use core::{fmt, ops::Deref};

pub use style::{Style, Styles};

/// A string that may have color and/or style applied to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColoredString<const N: usize> {
    input: StaticString<N>,
    fgcolor: Option<Color>,
    bgcolor: Option<Color>,
    style: style::Style,
}

/// The trait that enables something to be given color.
///
/// You can use `colored` effectively simply by importing this trait
/// and then using its methods on `String` and `&str`.
#[allow(missing_docs)]
pub trait Colorize<const N: usize> {
    // Font Colors
    fn black(self) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.color(Color::Black)
    }

    fn red(self) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.color(Color::Red)
    }

    fn green(self) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.color(Color::Green)
    }

    fn yellow(self) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.color(Color::Yellow)
    }

    fn blue(self) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.color(Color::Blue)
    }

    fn magenta(self) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.color(Color::Magenta)
    }

    fn purple(self) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.color(Color::Magenta)
    }

    fn cyan(self) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.color(Color::Cyan)
    }

    fn white(self) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.color(Color::White)
    }

    fn bright_black(self) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.color(Color::BrightBlack)
    }

    fn bright_red(self) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.color(Color::BrightRed)
    }

    fn bright_green(self) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.color(Color::BrightGreen)
    }

    fn bright_yellow(self) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.color(Color::BrightYellow)
    }

    fn bright_blue(self) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.color(Color::BrightBlue)
    }

    fn bright_magenta(self) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.color(Color::BrightMagenta)
    }

    fn bright_purple(self) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.color(Color::BrightMagenta)
    }

    fn bright_cyan(self) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.color(Color::BrightCyan)
    }

    fn bright_white(self) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.color(Color::BrightWhite)
    }

    fn truecolor(self, r: u8, g: u8, b: u8) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.color(Color::TrueColor { r, g, b })
    }

    fn custom_color(self, color: CustomColor) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.color(Color::TrueColor {
            r: color.r,
            g: color.g,
            b: color.b,
        })
    }

    fn color<S: Into<Color>>(self, color: S) -> Result<ColoredString<N>, CapacityError<N>>;
    // Background Colors

    fn on_black(self) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.on_color(Color::Black)
    }

    fn on_red(self) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.on_color(Color::Red)
    }

    fn on_green(self) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.on_color(Color::Green)
    }

    fn on_yellow(self) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.on_color(Color::Yellow)
    }

    fn on_blue(self) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.on_color(Color::Blue)
    }

    fn on_magenta(self) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.on_color(Color::Magenta)
    }

    fn on_purple(self) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.on_color(Color::Magenta)
    }

    fn on_cyan(self) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.on_color(Color::Cyan)
    }

    fn on_white(self) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.on_color(Color::White)
    }

    fn on_bright_black(self) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.on_color(Color::BrightBlack)
    }

    fn on_bright_red(self) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.on_color(Color::BrightRed)
    }

    fn on_bright_green(self) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.on_color(Color::BrightGreen)
    }

    fn on_bright_yellow(self) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.on_color(Color::BrightYellow)
    }

    fn on_bright_blue(self) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.on_color(Color::BrightBlue)
    }

    fn on_bright_magenta(self) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.on_color(Color::BrightMagenta)
    }

    fn on_bright_purple(self) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.on_color(Color::BrightMagenta)
    }

    fn on_bright_cyan(self) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.on_color(Color::BrightCyan)
    }

    fn on_bright_white(self) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.on_color(Color::BrightWhite)
    }

    fn on_truecolor(self, r: u8, g: u8, b: u8) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.on_color(Color::TrueColor { r, g, b })
    }

    fn on_custom_color(self, color: CustomColor) -> Result<ColoredString<N>, CapacityError<N>>
    where
        Self: Sized,
    {
        self.on_color(Color::TrueColor {
            r: color.r,
            g: color.g,
            b: color.b,
        })
    }

    fn on_color<S: Into<Color>>(self, color: S) -> Result<ColoredString<N>, CapacityError<N>>;
    // Styles

    fn clear(self) -> Result<ColoredString<N>, CapacityError<N>>;

    fn normal(self) -> Result<ColoredString<N>, CapacityError<N>>;

    fn bold(self) -> Result<ColoredString<N>, CapacityError<N>>;

    fn dimmed(self) -> Result<ColoredString<N>, CapacityError<N>>;

    fn italic(self) -> Result<ColoredString<N>, CapacityError<N>>;

    fn underline(self) -> Result<ColoredString<N>, CapacityError<N>>;

    fn blink(self) -> Result<ColoredString<N>, CapacityError<N>>;

    /// Historical name of `Colorize::reversed`. May be removed in a future version. Please use
    /// `Colorize::reversed` instead
    fn reverse(self) -> Result<ColoredString<N>, CapacityError<N>>;

    /// This should be preferred to `Colorize::reverse`.
    fn reversed(self) -> Result<ColoredString<N>, CapacityError<N>>;

    fn hidden(self) -> Result<ColoredString<N>, CapacityError<N>>;

    fn strikethrough(self) -> Result<ColoredString<N>, CapacityError<N>>;
}

impl<const N: usize> ColoredString<N> {
    /// Get the current background color applied.
    ///
    /// ```rust
    /// # use colored::*;
    /// let cstr = "".blue();
    /// assert_eq!(cstr.fgcolor(), Some(Color::Blue));
    /// let cstr = cstr.clear();
    /// assert_eq!(cstr.fgcolor(), None);
    /// ```
    pub fn fgcolor(&self) -> Option<Color> {
        self.fgcolor.as_ref().copied()
    }

    /// Get the current background color applied.
    ///
    /// ```rust
    /// # use colored::*;
    /// let cstr = "".on_blue();
    /// assert_eq!(cstr.bgcolor(), Some(Color::Blue));
    /// let cstr = cstr.clear();
    /// assert_eq!(cstr.bgcolor(), None);
    /// ```
    pub fn bgcolor(&self) -> Option<Color> {
        self.bgcolor.as_ref().copied()
    }

    /// Get the current [`Style`] which can be check if it contains a [`Styles`].
    ///
    /// ```rust
    /// # use colored::*;
    /// let colored = "".bold().italic();
    /// assert_eq!(colored.style().contains(Styles::Bold), true);
    /// assert_eq!(colored.style().contains(Styles::Italic), true);
    /// assert_eq!(colored.style().contains(Styles::Dimmed), false);
    /// ```
    pub fn style(&self) -> style::Style {
        self.style
    }

    /// Checks if the colored string has no color or styling.
    ///
    /// ```rust
    /// # use colored::*;
    /// let cstr = "".red();
    /// assert_eq!(cstr.is_plain(), false);
    /// let cstr = cstr.clear();
    /// assert_eq!(cstr.is_plain(), true);
    /// ```
    pub fn is_plain(&self) -> bool {
        self.bgcolor.is_none() && self.fgcolor.is_none() && self.style == style::CLEAR
    }

    #[cfg(not(feature = "no-color"))]
    fn has_colors(&self) -> bool {
        control::SHOULD_COLORIZE.should_colorize()
    }

    #[cfg(feature = "no-color")]
    fn has_colors(&self) -> bool {
        false
    }

    #[allow(dead_code)]
    const COMPUTE_STYLE_MAX_LEN: usize = Style::TO_STR_MAX_LEN + Color::TO_STR_MAX_LEN * 2 + 4;
    fn compute_style(
        &self,
    ) -> StaticString<{ Style::TO_STR_MAX_LEN + Color::TO_STR_MAX_LEN * 2 + 4 }> {
        if !self.has_colors() || self.is_plain() {
            return StaticString::new();
        }

        let mut res = StaticString::from_str("\x1B[");
        let mut has_wrote = if self.style != style::CLEAR {
            res.push_str(&self.style.to_str());
            true
        } else {
            false
        };

        if let Some(ref bgcolor) = self.bgcolor {
            if has_wrote {
                res.push(';');
            }

            res.push_str(&bgcolor.to_bg_str());
            has_wrote = true;
        }

        if let Some(ref fgcolor) = self.fgcolor {
            if has_wrote {
                res.push(';');
            }

            res.push_str(&fgcolor.to_fg_str());
        }

        res.push('m');
        res
    }
}

impl<const N: usize> Default for ColoredString<N> {
    fn default() -> Self {
        ColoredString {
            input: StaticString::default(),
            fgcolor: None,
            bgcolor: None,
            style: style::CLEAR,
        }
    }
}

impl<const N: usize> Deref for ColoredString<N> {
    type Target = str;
    fn deref(&self) -> &str {
        &self.input
    }
}

impl<'a, const N: usize> TryFrom<&'a str> for ColoredString<N> {
    type Error = CapacityError<N>;

    fn try_from(value: &'a str) -> Result<Self, Self::Error> {
        let str = StaticString::try_from_str(value)?;
        Ok(Self {
            input: str,
            ..Self::default()
        })
    }
}

impl<const N: usize> Colorize<N> for ColoredString<N> {
    fn color<S: Into<Color>>(mut self, color: S) -> Result<ColoredString<N>, CapacityError<N>> {
        self.fgcolor = Some(color.into());
        Ok(self)
    }
    fn on_color<S: Into<Color>>(mut self, color: S) -> Result<ColoredString<N>, CapacityError<N>> {
        self.bgcolor = Some(color.into());
        Ok(self)
    }

    fn clear(self) -> Result<ColoredString<N>, CapacityError<N>> {
        Ok(ColoredString {
            input: self.input,
            ..ColoredString::default()
        })
    }
    fn normal(self) -> Result<ColoredString<N>, CapacityError<N>> {
        self.clear()
    }
    fn bold(mut self) -> Result<ColoredString<N>, CapacityError<N>> {
        self.style.add(style::Styles::Bold);
        Ok(self)
    }
    fn dimmed(mut self) -> Result<ColoredString<N>, CapacityError<N>> {
        self.style.add(style::Styles::Dimmed);
        Ok(self)
    }
    fn italic(mut self) -> Result<ColoredString<N>, CapacityError<N>> {
        self.style.add(style::Styles::Italic);
        Ok(self)
    }
    fn underline(mut self) -> Result<ColoredString<N>, CapacityError<N>> {
        self.style.add(style::Styles::Underline);
        Ok(self)
    }
    fn blink(mut self) -> Result<ColoredString<N>, CapacityError<N>> {
        self.style.add(style::Styles::Blink);
        Ok(self)
    }
    fn reverse(self) -> Result<ColoredString<N>, CapacityError<N>> {
        self.reversed()
    }
    fn reversed(mut self) -> Result<ColoredString<N>, CapacityError<N>> {
        self.style.add(style::Styles::Reversed);
        Ok(self)
    }
    fn hidden(mut self) -> Result<ColoredString<N>, CapacityError<N>> {
        self.style.add(style::Styles::Hidden);
        Ok(self)
    }
    fn strikethrough(mut self) -> Result<ColoredString<N>, CapacityError<N>> {
        self.style.add(style::Styles::Strikethrough);
        Ok(self)
    }
}

impl<'a, const N: usize> Colorize<N> for &'a str {
    fn color<S: Into<Color>>(self, color: S) -> Result<ColoredString<N>, CapacityError<N>> {
        let input = StaticString::try_from(self)?;
        Ok(ColoredString {
            fgcolor: Some(color.into()),
            input,
            ..ColoredString::default()
        })
    }

    fn on_color<S: Into<Color>>(self, color: S) -> Result<ColoredString<N>, CapacityError<N>> {
        let input = StaticString::try_from(self)?;
        Ok(ColoredString {
            input,
            bgcolor: Some(color.into()),
            ..ColoredString::default()
        })
    }

    fn clear(self) -> Result<ColoredString<N>, CapacityError<N>> {
        let input = StaticString::try_from(self)?;
        Ok(ColoredString {
            input,
            style: style::CLEAR,
            ..ColoredString::default()
        })
    }
    fn normal(self) -> Result<ColoredString<N>, CapacityError<N>> {
        self.clear()
    }
    fn bold(self) -> Result<ColoredString<N>, CapacityError<N>> {
        ColoredString::try_from(self)?.bold()
    }
    fn dimmed(self) -> Result<ColoredString<N>, CapacityError<N>> {
        ColoredString::try_from(self)?.dimmed()
    }
    fn italic(self) -> Result<ColoredString<N>, CapacityError<N>> {
        ColoredString::try_from(self)?.italic()
    }
    fn underline(self) -> Result<ColoredString<N>, CapacityError<N>> {
        ColoredString::try_from(self)?.underline()
    }
    fn blink(self) -> Result<ColoredString<N>, CapacityError<N>> {
        ColoredString::try_from(self)?.blink()
    }
    fn reverse(self) -> Result<ColoredString<N>, CapacityError<N>> {
        self.reversed()
    }
    fn reversed(self) -> Result<ColoredString<N>, CapacityError<N>> {
        ColoredString::try_from(self)?.reversed()
    }
    fn hidden(self) -> Result<ColoredString<N>, CapacityError<N>> {
        ColoredString::try_from(self)?.hidden()
    }
    fn strikethrough(self) -> Result<ColoredString<N>, CapacityError<N>> {
        ColoredString::try_from(self)?.strikethrough()
    }
}

impl<const N: usize> fmt::Display for ColoredString<N> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if !self.has_colors() || self.is_plain() {
            return <StaticString<N> as fmt::Display>::fmt(&self.input, f);
        }

        let style = self.compute_style();
        f.write_str(&style)?;

        if !self.has_colors() || self.is_plain() {
            f.write_str(&self.input)?;
        }

        // TODO: BoyScoutRule
        let reset = "\x1B[0m";
        let matches = self.input.match_indices(reset).map(|(idx, _)| idx);

        let mut any = false;
        let mut done = 0;
        for (_idx_in_matches, offset) in matches.enumerate() {
            any = true;
            let offset = offset + reset.len();
            f.write_str(&self.input[done..offset])?;
            f.write_str(&style)?;
            done = offset;
        }
        if !any {
            f.write_str(&self.input)?;
        }
        f.write_str("\x1B[0m")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting() {
        // respect the formatting. Escape sequence add some padding so >= 40
        assert!(format!("{:40}", "".blue()).len() >= 40);
        // both should be truncated to 1 char before coloring
        assert_eq!(
            format!("{:1.1}", "toto".blue()).len(),
            format!("{:1.1}", "1".blue()).len()
        )
    }

    #[test]
    fn it_works() {
        let toto = "toto";
        println!("{}", toto.red());
        println!("{}", String::from(toto).red());
        println!("{}", toto.blue());

        println!("blue style ****");
        println!("{}", toto.bold());
        println!("{}", "yeah ! Red bold !".red().bold());
        println!("{}", "yeah ! Yellow bold !".bold().yellow());
        println!("{}", toto.bold().blue());
        println!("{}", toto.blue().bold());
        println!("{}", toto.blue().bold().underline());
        println!("{}", toto.blue().italic());
        println!("******");
        println!("test clearing");
        println!("{}", "red cleared".red().clear());
        println!("{}", "bold cyan cleared".bold().cyan().clear());
        println!("******");
        println!("Bg tests");
        println!("{}", toto.green().on_blue());
        println!("{}", toto.on_magenta().yellow());
        println!("{}", toto.purple().on_yellow());
        println!("{}", toto.magenta().on_white());
        println!("{}", toto.cyan().on_green());
        println!("{}", toto.black().on_white());
        println!("******");
        println!("{}", toto.green());
        println!("{}", toto.yellow());
        println!("{}", toto.purple());
        println!("{}", toto.magenta());
        println!("{}", toto.cyan());
        println!("{}", toto.white());
        println!("{}", toto.white().red().blue().green());
        println!("{}", toto.truecolor(255, 0, 0));
        println!("{}", toto.truecolor(255, 255, 0));
        println!("{}", toto.on_truecolor(0, 80, 80));
        // uncomment to see term output
        // assert!(false)
    }

    #[test]
    fn compute_style_empty_string() {
        assert_eq!("", "".clear().compute_style());
    }

    #[cfg_attr(feature = "no-color", ignore)]
    #[test]
    fn compute_style_simple_fg_blue() {
        let blue = "\x1B[34m";

        assert_eq!(blue, "".blue().compute_style());
    }

    #[cfg_attr(feature = "no-color", ignore)]
    #[test]
    fn compute_style_simple_bg_blue() {
        let on_blue = "\x1B[44m";

        assert_eq!(on_blue, "".on_blue().compute_style());
    }

    #[cfg_attr(feature = "no-color", ignore)]
    #[test]
    fn compute_style_blue_on_blue() {
        let blue_on_blue = "\x1B[44;34m";

        assert_eq!(blue_on_blue, "".blue().on_blue().compute_style());
    }

    #[cfg_attr(feature = "no-color", ignore)]
    #[test]
    fn compute_style_simple_fg_bright_blue() {
        let blue = "\x1B[94m";

        assert_eq!(blue, "".bright_blue().compute_style());
    }

    #[cfg_attr(feature = "no-color", ignore)]
    #[test]
    fn compute_style_simple_bg_bright_blue() {
        let on_blue = "\x1B[104m";

        assert_eq!(on_blue, "".on_bright_blue().compute_style());
    }

    #[cfg_attr(feature = "no-color", ignore)]
    #[test]
    fn compute_style_bright_blue_on_bright_blue() {
        let blue_on_blue = "\x1B[104;94m";

        assert_eq!(
            blue_on_blue,
            "".bright_blue().on_bright_blue().compute_style()
        );
    }

    #[cfg_attr(feature = "no-color", ignore)]
    #[test]
    fn compute_style_simple_bold() {
        let bold = "\x1B[1m";

        assert_eq!(bold, "".bold().compute_style());
    }

    #[cfg_attr(feature = "no-color", ignore)]
    #[test]
    fn compute_style_blue_bold() {
        let blue_bold = "\x1B[1;34m";

        assert_eq!(blue_bold, "".blue().bold().compute_style());
    }

    #[cfg_attr(feature = "no-color", ignore)]
    #[test]
    fn compute_style_blue_bold_on_blue() {
        let blue_bold_on_blue = "\x1B[1;44;34m";

        assert_eq!(
            blue_bold_on_blue,
            "".blue().bold().on_blue().compute_style()
        );
    }

    #[test]
    fn escape_reset_sequence_spec_should_do_nothing_on_empty_strings() {
        let style = ColoredString::default();
        let expected = String::new();

        let output = style.escape_inner_reset_sequences();

        assert_eq!(expected, output);
    }

    #[test]
    fn escape_reset_sequence_spec_should_do_nothing_on_string_with_no_reset() {
        let style = ColoredString {
            input: String::from("hello world !"),
            ..ColoredString::default()
        };

        let expected = String::from("hello world !");
        let output = style.escape_inner_reset_sequences();

        assert_eq!(expected, output);
    }

    #[cfg_attr(feature = "no-color", ignore)]
    #[test]
    fn escape_reset_sequence_spec_should_replace_inner_reset_sequence_with_current_style() {
        let input = format!("start {} end", String::from("hello world !").red());
        let style = input.blue();

        let output = style.escape_inner_reset_sequences();
        let blue = "\x1B[34m";
        let red = "\x1B[31m";
        let reset = "\x1B[0m";
        let expected = format!("start {}hello world !{}{} end", red, reset, blue);
        assert_eq!(expected, output);
    }

    #[cfg_attr(feature = "no-color", ignore)]
    #[test]
    fn escape_reset_sequence_spec_should_replace_multiple_inner_reset_sequences_with_current_style()
    {
        let italic_str = String::from("yo").italic();
        let input = format!(
            "start 1:{} 2:{} 3:{} end",
            italic_str, italic_str, italic_str
        );
        let style = input.blue();

        let output = style.escape_inner_reset_sequences();
        let blue = "\x1B[34m";
        let italic = "\x1B[3m";
        let reset = "\x1B[0m";
        let expected = format!(
            "start 1:{}yo{}{} 2:{}yo{}{} 3:{}yo{}{} end",
            italic, reset, blue, italic, reset, blue, italic, reset, blue
        );

        println!("first: {}\nsecond: {}", expected, output);

        assert_eq!(expected, output);
    }

    #[test]
    fn color_fn() {
        assert_eq!("blue".blue(), "blue".color("blue"))
    }

    #[test]
    fn on_color_fn() {
        assert_eq!("blue".on_blue(), "blue".on_color("blue"))
    }

    #[test]
    fn bright_color_fn() {
        assert_eq!("blue".bright_blue(), "blue".color("bright blue"))
    }

    #[test]
    fn on_bright_color_fn() {
        assert_eq!("blue".on_bright_blue(), "blue".on_color("bright blue"))
    }

    #[test]
    fn exposing_tests() {
        let cstring = "".red();
        assert_eq!(cstring.fgcolor(), Some(Color::Red));
        assert_eq!(cstring.bgcolor(), None);

        let cstring = cstring.clear();
        assert_eq!(cstring.fgcolor(), None);
        assert_eq!(cstring.bgcolor(), None);

        let cstring = cstring.blue().on_bright_yellow();
        assert_eq!(cstring.fgcolor(), Some(Color::Blue));
        assert_eq!(cstring.bgcolor(), Some(Color::BrightYellow));

        let cstring = cstring.bold().italic();
        assert_eq!(cstring.fgcolor(), Some(Color::Blue));
        assert_eq!(cstring.bgcolor(), Some(Color::BrightYellow));
        assert_eq!(cstring.style().contains(Styles::Bold), true);
        assert_eq!(cstring.style().contains(Styles::Italic), true);
        assert_eq!(cstring.style().contains(Styles::Dimmed), false);
    }
}
