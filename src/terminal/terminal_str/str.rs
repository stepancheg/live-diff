use std::fmt;
use std::fmt::Debug;
use std::ops::Deref;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::terminal::terminal_str::chars::TerminalChars;
use crate::terminal::terminal_str::string::TerminalString;
use crate::terminal::terminal_str::width_len::WidthLen;
use crate::terminal::width::Width;

/// Borrowed text safe to print, with its display width cached. No control characters.
#[derive(Clone, Copy, Eq, PartialEq, derive_more::Display)]
#[display("{content}")]
pub(crate) struct TerminalStr<'a> {
    content: &'a str,
    width: Width,
}

impl<'a> Debug for TerminalStr<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Debug::fmt(&self.content, f)
    }
}

impl<'a> PartialEq<str> for TerminalStr<'a> {
    fn eq(&self, other: &str) -> bool {
        self.content == other
    }
}

impl<'a> PartialEq<&str> for TerminalStr<'a> {
    fn eq(&self, other: &&str) -> bool {
        self.content == *other
    }
}

impl<'a> PartialEq<TerminalStr<'a>> for str {
    fn eq(&self, other: &TerminalStr<'a>) -> bool {
        self == other.content
    }
}

impl<'a> PartialEq<TerminalStr<'a>> for &str {
    fn eq(&self, other: &TerminalStr<'a>) -> bool {
        *self == other.content
    }
}

impl<'a> TerminalStr<'a> {
    /// Only use in const context.
    pub(crate) const fn const_new_ascii(s: &'a str) -> TerminalStr<'a> {
        let bytes = s.as_bytes();
        let mut i = 0;
        let mut width = 0u32;
        while i < bytes.len() {
            let b = bytes[i];
            assert!(b.is_ascii() && !b.is_ascii_control(), "invalid char");
            let Some(next) = width.checked_add(1) else {
                panic!("ascii terminal str longer than u32");
            };
            width = next;
            i += 1;
        }
        TerminalStr {
            content: s,
            width: Width(width),
        }
    }

    pub(crate) const EMPTY: TerminalStr<'static> = TerminalStr::const_new_ascii("");
    pub(crate) const SPACE: TerminalStr<'static> = TerminalStr::const_new_ascii(" ");

    pub(crate) fn unchecked_new(s: &'a str, width: Width) -> TerminalStr<'a> {
        TerminalStr { content: s, width }
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) fn new(s: &'a str) -> Option<TerminalStr<'a>> {
        if s.chars().any(|c| c.is_control()) {
            return None;
        }
        let width = Width::try_from(s.width_cjk()).ok()?;
        Some(TerminalStr { content: s, width })
    }

    #[expect(dead_code)]
    pub(crate) fn repeat(&self, repeat: usize) -> TerminalString {
        let mut r = TerminalString::with_capacity(self.content.len().saturating_mul(repeat));
        for _ in 0..repeat {
            r.push_str(*self);
        }
        r
    }

    pub(crate) fn width(self) -> Width {
        self.width
    }

    pub(crate) fn width_len(self) -> WidthLen {
        WidthLen {
            width: self.width,
            len: self.content.len(),
        }
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) fn trim_start(self) -> TerminalStr<'a> {
        let mut chars = self.chars();
        loop {
            let s = chars.as_str();
            let Some(first) = chars.next() else {
                return s;
            };
            if !first.is_whitespace() {
                return s;
            }
        }
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) fn trim_end(self) -> TerminalStr<'a> {
        let mut chars = self.chars();
        loop {
            let s = chars.as_str();
            let Some(last) = chars.next_back() else {
                return s;
            };
            if !last.is_whitespace() {
                return s;
            }
        }
    }

    pub(crate) fn to_owned(self) -> TerminalString {
        TerminalString::from_str_and_width(self.content, self.width)
    }

    pub(crate) fn as_str(self) -> &'a str {
        self.content
    }

    pub(crate) fn chars(self) -> TerminalChars<'a> {
        TerminalChars::new(self)
    }

    /// Prefix of at most `count` columns, scanning from the start.
    /// A `count` inside a grapheme stops before it.
    fn do_take(self, count: Width) -> TerminalStr<'a> {
        let mut cum = Width(0);
        let mut split_idx = 0;
        for (i, g) in self.content.grapheme_indices(true) {
            let w = Width(g.width_cjk() as u32);
            if cum + w > count {
                break;
            }
            cum += w;
            split_idx = i + g.len();
        }
        TerminalStr {
            content: &self.content[..split_idx],
            width: cum,
        }
    }

    /// Suffix of at most `count` columns, scanning from the end.
    /// A `count` inside a grapheme stops before it.
    fn do_rtake(self, count: Width) -> TerminalStr<'a> {
        let mut cum = Width(0);
        let mut split_idx = self.content.len();
        for (i, g) in self.content.grapheme_indices(true).rev() {
            let w = Width(g.width_cjk() as u32);
            if cum + w > count {
                break;
            }
            cum += w;
            split_idx = i;
        }
        TerminalStr {
            content: &self.content[split_idx..],
            width: cum,
        }
    }

    /// Prefix of at most `count` columns.
    /// `count` at least this width returns `self`.
    /// A `count` inside a grapheme stops before it, so the prefix can be shorter than `count`.
    #[cfg_attr(not(test), expect(dead_code))]
    fn take(self, count: Width) -> TerminalStr<'a> {
        if count == Width(0) {
            TerminalStr::EMPTY
        } else if count >= self.width {
            self
        } else if count.0 * 2 < self.width.0 {
            self.do_take(count)
        } else {
            // Discarded tail is shorter, so scan from the end.
            let suffix = self.do_rtake(self.width - count);
            let (prefix, _) = self.split_at_unchecked(WidthLen {
                width: self.width - suffix.width,
                len: self.content.len() - suffix.content.len(),
            });
            if prefix.width <= count {
                prefix
            } else {
                // `count` falls inside this grapheme.
                let mut chars = prefix.chars();
                chars.next_back();
                chars.as_str()
            }
        }
    }

    /// Suffix of at most `count` columns.
    /// `count` at least this width returns `self`.
    /// A `count` inside a grapheme stops before it, so the suffix can be shorter than `count`.
    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) fn rtake(self, count: Width) -> TerminalStr<'a> {
        if count == Width(0) {
            TerminalStr::EMPTY
        } else if count >= self.width {
            self
        } else if count.0 * 2 < self.width.0 {
            self.do_rtake(count)
        } else {
            // Discarded head is shorter, so scan from the start.
            let prefix = self.do_take(self.width - count);
            let (_, suffix) = self.split_at_unchecked(prefix.width_len());
            if suffix.width <= count {
                suffix
            } else {
                // `count` falls inside this grapheme.
                let mut chars = suffix.chars();
                chars.next();
                chars.as_str()
            }
        }
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) fn split_at(self, at: Width) -> (TerminalStr<'a>, TerminalStr<'a>) {
        let left = self.take(at);
        self.split_at_unchecked(left.width_len())
    }

    pub(crate) fn split_at_unchecked(self, at: WidthLen) -> (TerminalStr<'a>, TerminalStr<'a>) {
        let (a, b) = self.content.split_at(at.len);
        let a = TerminalStr::unchecked_new(a, at.width);
        let b = TerminalStr::unchecked_new(b, self.width - at.width);
        (a, b)
    }

    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) fn wrap(self, max_width: Width) -> Vec<TerminalStr<'a>> {
        if max_width == Width(0) {
            let mut result = Vec::new();
            result.push(self);
            return result;
        }
        let mut result = Vec::new();
        let mut remaining = self;
        while remaining.width > Width(0) {
            let (left, right) = remaining.split_at(max_width);
            result.push(left);
            remaining = right;
        }
        result
    }
}

impl<'a> Deref for TerminalStr<'a> {
    type Target = str;

    fn deref(&self) -> &str {
        self.as_str()
    }
}

#[cfg(test)]
mod tests {
    use crate::terminal::terminal_str::str::TerminalStr;
    use crate::terminal::width::Width;

    #[test]
    fn test_take_iterate_from_start() {
        // "a日xxxxx" = width 8, take(2): count < width/2, iterate from start
        // width 2 falls in middle of "日" (can't split), returns "a" (1)
        let s = TerminalStr::new("a日xxxxx").unwrap();
        let taken = s.take(Width(2));
        assert_eq!(Width(1), taken.width());
        assert_eq!("a", taken.as_str());
    }

    #[test]
    fn test_take_iterate_from_end() {
        // "xxxxx日a" = width 8, take(6): count >= width/2, iterate from end
        // width 6 falls in middle of "日" (can't split), returns "xxxxx" (5)
        let s = TerminalStr::new("xxxxx日a").unwrap();
        let taken = s.take(Width(6));
        assert_eq!(Width(5), taken.width());
        assert_eq!("xxxxx", taken.as_str());
    }

    #[test]
    fn test_rtake_iterate_from_end() {
        // "xxxxx日a" = width 8, rtake(2): count < width/2, iterate from end
        // width 2 falls in middle of "日" (can't split), returns "a" (1)
        let s = TerminalStr::new("xxxxx日a").unwrap();
        let taken = s.rtake(Width(2));
        assert_eq!(Width(1), taken.width());
        assert_eq!("a", taken.as_str());
    }

    #[test]
    fn test_rtake_iterate_from_start() {
        // "a日xxxxx" = width 8, rtake(6): count >= width/2, iterate from start
        // width 6 falls in middle of "日" (can't split), returns "xxxxx" (5)
        let s = TerminalStr::new("a日xxxxx").unwrap();
        let taken = s.rtake(Width(6));
        assert_eq!(Width(5), taken.width());
        assert_eq!("xxxxx", taken.as_str());
    }

    #[test]
    fn test_do_take_middle_of_char() {
        // "xxxxx日a" = width 8. do_take(6) scans from the start.
        // width 6 falls inside "日", so the prefix stops before it.
        let s = TerminalStr::new("xxxxx日a").unwrap();
        let taken = s.do_take(Width(6));
        assert_eq!(Width(5), taken.width());
        assert_eq!("xxxxx", taken.as_str());
    }

    #[test]
    fn test_do_rtake_middle_of_char() {
        // "a日xxxxx" = width 8. do_rtake(6) scans from the end.
        // width 6 falls inside "日", so the suffix starts after it.
        let s = TerminalStr::new("a日xxxxx").unwrap();
        let taken = s.do_rtake(Width(6));
        assert_eq!(Width(5), taken.width());
        assert_eq!("xxxxx", taken.as_str());
    }

    #[test]
    fn test_split_at_middle_of_char_from_start() {
        // "a日xxxxx" = width 8, split_at(2) falls inside "日".
        let s = TerminalStr::new("a日xxxxx").unwrap();
        let (left, right) = s.split_at(Width(2));
        assert_eq!("a", left.as_str());
        assert_eq!(Width(1), left.width());
        assert_eq!("日xxxxx", right.as_str());
        assert_eq!(Width(7), right.width());
    }

    #[test]
    fn test_split_at_middle_of_char_from_end() {
        // "xxxxx日a" = width 8, split_at(6) falls inside "日".
        let s = TerminalStr::new("xxxxx日a").unwrap();
        let (left, right) = s.split_at(Width(6));
        assert_eq!("xxxxx", left.as_str());
        assert_eq!(Width(5), left.width());
        assert_eq!("日a", right.as_str());
        assert_eq!(Width(3), right.width());
    }

    #[test]
    fn test_trim_start() {
        let s = TerminalStr::new("  hello").unwrap();
        assert_eq!("hello", s.trim_start());

        let s = TerminalStr::new("hello").unwrap();
        assert_eq!("hello", s.trim_start());

        let s = TerminalStr::new("  ").unwrap();
        assert_eq!("", s.trim_start());
    }

    #[test]
    fn test_trim_end() {
        let s = TerminalStr::new("hello  ").unwrap();
        assert_eq!("hello", s.trim_end());

        let s = TerminalStr::new("hello").unwrap();
        assert_eq!("hello", s.trim_end());

        let s = TerminalStr::new("  ").unwrap();
        assert_eq!("", s.trim_end());
    }

    #[test]
    fn test_wrap_empty_string_width_88() {
        let result = TerminalStr::EMPTY.wrap(Width(88));
        assert_eq!(0, result.len());
    }
}
