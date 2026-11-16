use ref_cast::ref_cast_custom;

use crate::terminal::terminal_str::str::TerminalStr;
use crate::terminal::terminal_str::width_len::WidthLen;
use crate::terminal::width::Width;

#[derive(Debug, derive_more::Display, ref_cast::RefCastCustom)]
#[repr(transparent)]
pub(crate) struct TerminalCharRef(str);

impl TerminalCharRef {
    #[ref_cast_custom]
    pub(crate) fn unchecked_new(content: &str) -> &TerminalCharRef;

    pub(crate) fn as_str(&self) -> TerminalStr<'_> {
        TerminalStr::unchecked_new(&self.0, self.width())
    }

    pub(crate) fn is_whitespace(&self) -> bool {
        self.0.chars().all(char::is_whitespace)
    }

    pub(crate) fn width(&self) -> Width {
        Width(unicode_width::UnicodeWidthStr::width_cjk(&self.0) as u32)
    }

    #[expect(dead_code)]
    pub(crate) fn width_len(&self) -> WidthLen {
        WidthLen {
            width: self.width(),
            len: self.as_str().len(),
        }
    }
}

impl<'a> PartialEq<char> for &'a TerminalCharRef {
    fn eq(&self, other: &char) -> bool {
        let mut iter = self.0.chars();
        let Some(c0) = iter.next() else {
            return false;
        };
        if iter.next().is_some() {
            return false;
        }
        c0 == *other
    }
}

impl<'a> PartialEq<&'a TerminalCharRef> for char {
    fn eq(&self, other: &&'a TerminalCharRef) -> bool {
        other == self
    }
}
