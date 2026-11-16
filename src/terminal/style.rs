use crate::terminal::color::TerminalColor;
use crate::terminal::parse::Parser;

fn push_color_sgr(s: &mut String, color: TerminalColor, foreground: bool) {
    match color {
        TerminalColor::Black => s.push_str(if foreground { "30" } else { "40" }),
        TerminalColor::Red => s.push_str(if foreground { "31" } else { "41" }),
        TerminalColor::Green => s.push_str(if foreground { "32" } else { "42" }),
        TerminalColor::Yellow => s.push_str(if foreground { "33" } else { "43" }),
        TerminalColor::Blue => s.push_str(if foreground { "34" } else { "44" }),
        TerminalColor::Magenta => s.push_str(if foreground { "35" } else { "45" }),
        TerminalColor::Cyan => s.push_str(if foreground { "36" } else { "46" }),
        TerminalColor::White => s.push_str(if foreground { "37" } else { "47" }),
        TerminalColor::BrightBlack => s.push_str(if foreground { "90" } else { "100" }),
        TerminalColor::BrightRed => s.push_str(if foreground { "91" } else { "101" }),
        TerminalColor::BrightGreen => s.push_str(if foreground { "92" } else { "102" }),
        TerminalColor::BrightYellow => s.push_str(if foreground { "93" } else { "103" }),
        TerminalColor::BrightBlue => s.push_str(if foreground { "94" } else { "104" }),
        TerminalColor::BrightMagenta => s.push_str(if foreground { "95" } else { "105" }),
        TerminalColor::BrightCyan => s.push_str(if foreground { "96" } else { "106" }),
        TerminalColor::BrightWhite => s.push_str(if foreground { "97" } else { "107" }),
        TerminalColor::Ansi256(n) => {
            if foreground {
                s.push_str("38;5;");
            } else {
                s.push_str("48;5;");
            }
            s.push_str(&n.to_string());
        }
        TerminalColor::Rgb(r, g, b) => {
            if foreground {
                s.push_str("38;2;");
            } else {
                s.push_str("48;2;");
            }
            s.push_str(&r.to_string());
            s.push(';');
            s.push_str(&g.to_string());
            s.push(';');
            s.push_str(&b.to_string());
        }
    }
}

/// SGR style for a span.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Style {
    /// `None` means the terminal default.
    pub(crate) fg: Option<TerminalColor>,
    /// `None` means the terminal default.
    pub(crate) bg: Option<TerminalColor>,
    pub(crate) bold: bool,
    pub(crate) dim: bool,
    pub(crate) italic: bool,
    pub(crate) underline: bool,
    pub(crate) reverse: bool,
    pub(crate) strikethrough: bool,
}

impl Style {
    /// Absolute SGR sequence for this style (`ESC [ 0; ... m`).
    /// The leading reset makes the sequence independent of the current terminal style.
    pub(crate) fn sgr_sequence(&self) -> String {
        let mut s = String::from("\x1b[0");
        if self.bold {
            s.push_str(";1");
        }
        if self.dim {
            s.push_str(";2");
        }
        if self.italic {
            s.push_str(";3");
        }
        if self.underline {
            s.push_str(";4");
        }
        if self.reverse {
            s.push_str(";7");
        }
        if self.strikethrough {
            s.push_str(";9");
        }
        if let Some(fg) = self.fg {
            s.push(';');
            push_color_sgr(&mut s, fg, true);
        }
        if let Some(bg) = self.bg {
            s.push(';');
            push_color_sgr(&mut s, bg, false);
        }
        s.push('m');
        s
    }

    pub(crate) fn plain() -> Style {
        Style {
            fg: None,
            bg: None,
            bold: false,
            dim: false,
            italic: false,
            underline: false,
            reverse: false,
            strikethrough: false,
        }
    }

    /// Apply a CSI SGR parameter list (`ESC [ ... m`).
    pub(crate) fn apply_sgr_to_style(&mut self, mut params: &[u32]) {
        while let Some((&p, rest)) = params.split_first() {
            params = rest;
            match p {
                0 => *self = Style::plain(),
                1 => self.bold = true,
                2 => self.dim = true,
                3 => self.italic = true,
                4 => self.underline = true,
                7 => self.reverse = true,
                9 => self.strikethrough = true,
                22 => {
                    self.bold = false;
                    self.dim = false;
                }
                23 => self.italic = false,
                24 => self.underline = false,
                27 => self.reverse = false,
                29 => self.strikethrough = false,
                30..=37 => self.fg = Some(TerminalColor::basic_color(p - 30, false)),
                38 => params = self.apply_extended_color(params, true),
                39 => self.fg = None,
                40..=47 => self.bg = Some(TerminalColor::basic_color(p - 40, false)),
                48 => params = self.apply_extended_color(params, false),
                49 => self.bg = None,
                90..=97 => self.fg = Some(TerminalColor::basic_color(p - 90, true)),
                100..=107 => self.bg = Some(TerminalColor::basic_color(p - 100, true)),
                _ => {}
            }
        }
    }

    /// `params` follows a 38 or 48. On a complete extended color, applies it and
    /// returns the tail. If the sequence is missing or not mode 2/5, returns
    /// `params` unchanged so those values are read as ordinary SGR codes.
    fn apply_extended_color<'a>(&mut self, params: &'a [u32], foreground: bool) -> &'a [u32] {
        let Some((&mode, after_mode)) = params.split_first() else {
            return params;
        };
        match mode {
            5 => {
                let Some((&n, rest)) = after_mode.split_first() else {
                    return params;
                };
                let color = TerminalColor::Ansi256(n as u8);
                if foreground {
                    self.fg = Some(color);
                } else {
                    self.bg = Some(color);
                }
                rest
            }
            2 => {
                // Semicolon form is `38;2;r;g;b`. A following SGR code must not be
                // eaten as a colorspace id.
                if after_mode.len() < 3 {
                    return params;
                }
                let r = after_mode[0];
                let g = after_mode[1];
                let b = after_mode[2];
                let color = TerminalColor::Rgb(r as u8, g as u8, b as u8);
                if foreground {
                    self.fg = Some(color);
                } else {
                    self.bg = Some(color);
                }
                &after_mode[3..]
            }
            _ => params,
        }
    }

    /// CSI body (after `ESC [`). Applies SGR when the final byte is `m`
    /// and consumes the sequence from `parser`.
    pub(crate) fn consume_csi(&mut self, parser: &mut Parser<'_>) {
        // `&str` is Copy, so this does not borrow `parser` across the update.
        let body = parser.rem_input;
        let bytes = body.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            if (0x40..=0x7e).contains(&b) {
                if b == b'm' {
                    if let Some(params) = Style::parse_sgr_params(&body[..i]) {
                        self.apply_sgr_to_style(&params);
                    }
                }
                parser.rem_input = &body[i + 1..];
                return;
            }
            i += 1;
        }
        parser.rem_input = "";
    }

    fn parse_sgr_params(body: &str) -> Option<Vec<u32>> {
        // `ESC [ m` is reset, the same as `ESC [ 0 m`.
        if body.is_empty() {
            let mut params = Vec::new();
            params.push(0);
            return Some(params);
        }
        let mut params = Vec::new();
        for part in body.split(';') {
            if part.is_empty() {
                params.push(0);
                continue;
            }
            let Ok(n) = part.parse::<u32>() else {
                return None;
            };
            params.push(n);
        }
        Some(params)
    }
}

#[cfg(test)]
mod tests {
    use crate::terminal::color::TerminalColor;
    use crate::terminal::style::Style;

    #[test]
    fn test_zero_resets() {
        let mut style = Style::plain();
        style.bold = true;
        style.fg = Some(TerminalColor::Red);
        style.apply_sgr_to_style(&[0]);
        assert_eq!(Style::plain(), style);
    }

    #[test]
    fn test_bold_and_fg() {
        let mut style = Style::plain();
        style.apply_sgr_to_style(&[1, 31]);
        assert_eq!(true, style.bold);
        assert_eq!(Some(TerminalColor::Red), style.fg);
    }

    #[test]
    fn test_reset_code() {
        let mut style = Style::plain();
        style.apply_sgr_to_style(&[1, 31, 0]);
        assert_eq!(Style::plain(), style);
    }

    #[test]
    fn test_ansi256_fg() {
        let mut style = Style::plain();
        style.apply_sgr_to_style(&[38, 5, 196]);
        assert_eq!(Some(TerminalColor::Ansi256(196)), style.fg);
    }

    #[test]
    fn test_rgb_fg() {
        let mut style = Style::plain();
        style.apply_sgr_to_style(&[38, 2, 1, 2, 3]);
        assert_eq!(Some(TerminalColor::Rgb(1, 2, 3)), style.fg);
    }

    #[test]
    fn test_bright_bg() {
        let mut style = Style::plain();
        style.apply_sgr_to_style(&[101]);
        assert_eq!(Some(TerminalColor::BrightRed), style.bg);
    }
}
