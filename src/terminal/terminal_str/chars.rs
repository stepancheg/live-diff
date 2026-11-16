use crate::terminal::terminal_str::char_indices::TerminalCharIndices;
use crate::terminal::terminal_str::char_ref::TerminalCharRef;
use crate::terminal::terminal_str::str::TerminalStr;

/// Iterator over graphemes of a `TerminalStr` with width tracking.
#[derive(Debug, Clone)]
pub(crate) struct TerminalChars<'a> {
    indices: TerminalCharIndices<'a>,
}

impl<'a> TerminalChars<'a> {
    pub(crate) fn new(s: TerminalStr<'a>) -> TerminalChars<'a> {
        TerminalChars {
            indices: TerminalCharIndices::new(s),
        }
    }

    pub(crate) fn as_str(&self) -> TerminalStr<'a> {
        self.indices.as_str()
    }
}

impl<'a> Iterator for TerminalChars<'a> {
    type Item = &'a TerminalCharRef;

    fn next(&mut self) -> Option<&'a TerminalCharRef> {
        Some(self.indices.next()?.1)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.indices.size_hint()
    }
}

impl<'a> DoubleEndedIterator for TerminalChars<'a> {
    fn next_back(&mut self) -> Option<&'a TerminalCharRef> {
        Some(self.indices.next_back()?.1)
    }
}
