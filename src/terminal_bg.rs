use crossterm::style::Color;
use terminal_colorsaurus::QueryOptions;
use terminal_colorsaurus::ThemeMode;
use terminal_colorsaurus::theme_mode;

/// Terminal background, from a color query.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TerminalBg {
    /// Dark background, or the query failed.
    Dark,
    /// Light background.
    Light,
}

impl Default for TerminalBg {
    fn default() -> TerminalBg {
        TerminalBg::Dark
    }
}

impl TerminalBg {
    /// Background for the status bar: light grey on white, dark grey on black.
    pub(crate) fn highlight_color(self) -> Color {
        match self {
            TerminalBg::Light => Color::Grey,
            TerminalBg::Dark => Color::DarkGrey,
        }
    }

    /// Absolute SGR for the status-bar background.
    /// Raw SGR keeps the bar colored when `NO_COLOR` is set.
    /// Grey is xterm color 7; DarkGrey is xterm color 8.
    pub(crate) fn status_bar_sgr(self) -> &'static str {
        match self.highlight_color() {
            Color::Grey => "\x1b[0;48;5;7m",
            Color::DarkGrey => "\x1b[0;48;5;8m",
            _ => "\x1b[0m",
        }
    }
}

/// Ask the terminal whether its background is light or dark.
/// Failure counts as dark.
pub(crate) fn query_terminal_bg() -> TerminalBg {
    match theme_mode(QueryOptions::default()) {
        Ok(ThemeMode::Light) => TerminalBg::Light,
        Ok(ThemeMode::Dark) | Err(_) => TerminalBg::Dark,
    }
}

#[cfg(test)]
mod tests {
    use crossterm::style::Color;

    use crate::terminal_bg::TerminalBg;

    #[test]
    fn test_highlight_color_follows_terminal_bg() {
        assert_eq!(Color::Grey, TerminalBg::Light.highlight_color());
        assert_eq!(Color::DarkGrey, TerminalBg::Dark.highlight_color());
    }
}
