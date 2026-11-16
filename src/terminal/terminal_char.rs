use compact_str::CompactString;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::terminal::width::Width;

/// One grapheme safe to print. Terminal width is 1 or 2.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TerminalChar {
    text: CompactString,
}

impl TerminalChar {
    /// Build a terminal char from exactly one non-control grapheme of width 1 or 2.
    pub(crate) fn new(s: &str) -> Option<TerminalChar> {
        if s.chars().any(|c| c.is_control()) {
            return None;
        }
        let mut graphemes = s.graphemes(true);
        if graphemes.next().is_none() || graphemes.next().is_some() {
            return None;
        }
        let width = Width(s.width_cjk() as u32);
        if width != Width(1) && width != Width(2) {
            return None;
        }
        Some(TerminalChar {
            text: CompactString::from(s),
        })
    }

    pub(crate) fn as_str(&self) -> &str {
        self.text.as_str()
    }

    pub(crate) fn width(&self) -> Width {
        Width(self.text.as_str().width_cjk() as u32)
    }
}

#[cfg(test)]
mod tests {
    use crate::terminal::terminal_char::TerminalChar;
    use crate::terminal::width::Width;

    #[test]
    fn test_ascii() {
        let c = TerminalChar::new("a").unwrap();
        assert_eq!("a", c.as_str());
        assert_eq!(Width(1), c.width());
    }

    #[test]
    fn test_double_width() {
        let c = TerminalChar::new("中").unwrap();
        assert_eq!("中", c.as_str());
        assert_eq!(Width(2), c.width());
    }

    #[test]
    fn test_combining_grapheme() {
        let c = TerminalChar::new("a\u{0308}").unwrap();
        assert_eq!("a\u{0308}", c.as_str());
        assert_eq!(Width(1), c.width());
    }

    #[test]
    fn test_empty() {
        assert_eq!(None, TerminalChar::new(""));
    }

    #[test]
    fn test_multiple_graphemes() {
        assert_eq!(None, TerminalChar::new("ab"));
    }

    #[test]
    fn test_control() {
        assert_eq!(None, TerminalChar::new("\n"));
        assert_eq!(None, TerminalChar::new("\t"));
    }

    #[test]
    fn test_zero_width() {
        assert_eq!(None, TerminalChar::new("\u{200B}"));
    }
}
