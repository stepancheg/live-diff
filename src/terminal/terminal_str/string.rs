use std::borrow::Cow;
use std::fmt::Display;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::terminal::terminal_char::TerminalChar;
use crate::terminal::terminal_str::str::TerminalStr;
use crate::terminal::width::Width;

#[derive(Clone, Eq, PartialEq, derive_more::Display, Debug)]
#[display("{content}")]
pub(crate) struct TerminalString {
    pub(crate) content: Cow<'static, str>,
    pub(crate) width: Width,
}

impl TerminalString {
    #[expect(dead_code)]
    pub(crate) const SPACE: TerminalString = TerminalString {
        content: Cow::Borrowed(" "),
        width: Width(1),
    };

    pub(crate) fn with_capacity(capacity: usize) -> TerminalString {
        TerminalString {
            content: Cow::Owned(String::with_capacity(capacity)),
            width: Width(0),
        }
    }

    pub(crate) fn from_str_and_width(s: &str, width: Width) -> TerminalString {
        TerminalString {
            content: Cow::Owned(s.to_owned()),
            width,
        }
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) fn new(s: impl Into<Cow<'static, str>>) -> Option<TerminalString> {
        let s = s.into();
        for c in s.chars() {
            if c.is_control() {
                return None;
            }
        }
        let width = Width::try_from(s.width_cjk()).ok()?;
        Some(TerminalString { content: s, width })
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) fn sanitize(s: impl Display) -> TerminalString {
        let mut content = String::new();
        for c in s.to_string().chars() {
            if c.is_control() {
                if !content.is_empty() && !content.ends_with(' ') {
                    content.push(' ');
                }
                continue;
            }
            content.push(c);
        }
        let width = match Width::try_from(content.width_cjk()) {
            Ok(width) => width,
            Err(_) => Width(0),
        };
        TerminalString {
            content: Cow::Owned(content),
            width,
        }
    }

    /// Display width in terminal columns.
    pub(crate) fn width(&self) -> Width {
        self.width
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.width == Width(0)
    }

    pub(crate) fn as_ref(&self) -> TerminalStr<'_> {
        TerminalStr::unchecked_new(&self.content, self.width)
    }

    pub(crate) fn push_str(&mut self, s: TerminalStr) {
        self.content.to_mut().push_str(s.as_str());
        self.width += s.width();
    }

    pub(crate) fn push(&mut self, c: TerminalChar) {
        self.content.to_mut().push_str(c.as_str());
        self.width += c.width();
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) fn pop_char(&mut self) -> Option<TerminalChar> {
        let pos = self.content.grapheme_indices(true).next_back()?.0;
        let term_char = TerminalChar::new(&self.content[pos..])?;
        self.content.to_mut().truncate(pos);
        self.width -= term_char.width();
        Some(term_char)
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) fn truncate(&mut self, width: Width) {
        if width == Width(0) {
            *self = TerminalString::with_capacity(0);
            return;
        }
        while self.width > width {
            self.pop_char();
        }
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) fn rtruncate(&mut self, width: Width) {
        let Some(truncate) = self.width.checked_sub(width) else {
            return;
        };
        if truncate.0 == 0 {
            return;
        }
        *self = self.as_ref().rtake(width).to_owned();
    }
}

impl From<TerminalChar> for TerminalString {
    fn from(c: TerminalChar) -> TerminalString {
        let width = c.width();
        TerminalString {
            content: Cow::Owned(c.as_str().to_owned()),
            width,
        }
    }
}

impl<'a> PartialEq<TerminalStr<'a>> for TerminalString {
    fn eq(&self, other: &TerminalStr<'a>) -> bool {
        self.as_ref() == *other
    }
}

impl<'a> PartialEq<TerminalString> for TerminalStr<'a> {
    fn eq(&self, other: &TerminalString) -> bool {
        *self == other.as_ref()
    }
}

impl<'a> PartialEq<&'a str> for TerminalString {
    fn eq(&self, other: &&'a str) -> bool {
        self.as_ref() == *other
    }
}

impl<'a> PartialEq<TerminalString> for &'a str {
    fn eq(&self, other: &TerminalString) -> bool {
        *self == other.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use crate::terminal::terminal_char::TerminalChar;
    use crate::terminal::terminal_str::str::TerminalStr;
    use crate::terminal::terminal_str::string::TerminalString;
    use crate::terminal::width::Width;

    #[test]
    fn test_pop_char() {
        let mut s = TerminalString::new("abc").unwrap();
        let popped = s.pop_char();
        assert_eq!(Some(TerminalChar::new("c").unwrap()), popped);
        assert_eq!(TerminalStr::new("ab").unwrap(), s);
    }

    #[test]
    fn test_string_truncate() {
        let mut s = TerminalString::new("abc").unwrap();
        s.truncate(Width(99));
        assert_eq!(TerminalStr::new("abc").unwrap(), s);
        s.truncate(Width(2));
        assert_eq!(TerminalStr::new("ab").unwrap(), s);
        s.truncate(Width(0));
        assert_eq!(TerminalStr::EMPTY, s);
    }

    #[test]
    fn test_string_rtruncate() {
        let mut s = TerminalString::new("abc").unwrap();
        s.rtruncate(Width(99));
        assert_eq!(TerminalStr::new("abc").unwrap(), s);
        s.rtruncate(Width(2));
        assert_eq!(TerminalStr::new("bc").unwrap(), s);
        s.rtruncate(Width(0));
        assert_eq!(TerminalStr::EMPTY, s);
    }

    #[test]
    fn test_sanitize_replaces_controls_with_space() {
        let s = TerminalString::sanitize("a\nb");
        assert_eq!("a b", s.as_ref());
    }
}
