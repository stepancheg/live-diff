/// ANSI color carried by a span.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TerminalColor {
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
    Ansi256(u8),
    Rgb(u8, u8, u8),
}

impl TerminalColor {
    /// Map an SGR basic-color index (0–7) to a standard or bright color.
    pub(crate) fn basic_color(index: u32, bright: bool) -> TerminalColor {
        match (index, bright) {
            (0, false) => TerminalColor::Black,
            (1, false) => TerminalColor::Red,
            (2, false) => TerminalColor::Green,
            (3, false) => TerminalColor::Yellow,
            (4, false) => TerminalColor::Blue,
            (5, false) => TerminalColor::Magenta,
            (6, false) => TerminalColor::Cyan,
            (7, false) => TerminalColor::White,
            (0, true) => TerminalColor::BrightBlack,
            (1, true) => TerminalColor::BrightRed,
            (2, true) => TerminalColor::BrightGreen,
            (3, true) => TerminalColor::BrightYellow,
            (4, true) => TerminalColor::BrightBlue,
            (5, true) => TerminalColor::BrightMagenta,
            (6, true) => TerminalColor::BrightCyan,
            _ => TerminalColor::BrightWhite,
        }
    }
}
