use unicode_segmentation::GraphemeIndices;
use unicode_segmentation::UnicodeSegmentation;

use crate::terminal::terminal_str::char_ref::TerminalCharRef;
use crate::terminal::terminal_str::str::TerminalStr;
use crate::terminal::terminal_str::width_len::WidthLen;
use crate::terminal::width::Width;

/// Iterator returning pairs of (offset, char) for [`TerminalStr`].
#[derive(Debug, Clone)]
pub(crate) struct TerminalCharIndices<'a> {
    inner: GraphemeIndices<'a>,
    offset: Width,
    back_offset: Width,
}

impl<'a> TerminalCharIndices<'a> {
    pub(crate) fn new(s: TerminalStr<'a>) -> TerminalCharIndices<'a> {
        TerminalCharIndices {
            inner: s.as_str().grapheme_indices(true),
            offset: Width(0),
            back_offset: s.width(),
        }
    }

    pub(crate) fn as_str(&self) -> TerminalStr<'a> {
        TerminalStr::unchecked_new(self.inner.as_str(), self.back_offset - self.offset)
    }
}

impl<'a> Iterator for TerminalCharIndices<'a> {
    type Item = (WidthLen, &'a TerminalCharRef);

    fn next(&mut self) -> Option<(WidthLen, &'a TerminalCharRef)> {
        let (offset, g) = self.inner.next()?;
        let c = TerminalCharRef::unchecked_new(g);
        let offset = WidthLen {
            width: self.offset,
            len: offset,
        };
        self.offset = self.offset + c.width();
        Some((offset, c))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<'a> DoubleEndedIterator for TerminalCharIndices<'a> {
    fn next_back(&mut self) -> Option<(WidthLen, &'a TerminalCharRef)> {
        let (offset, g) = self.inner.next_back()?;
        let c = TerminalCharRef::unchecked_new(g);
        let w = c.width();
        self.back_offset -= w;
        let ret = WidthLen {
            width: self.back_offset,
            len: offset,
        };
        Some((ret, c))
    }
}

#[cfg(test)]
mod tests {
    use crate::terminal::terminal_str::char_indices::TerminalCharIndices;
    use crate::terminal::terminal_str::str::TerminalStr;

    #[test]
    fn test_char_indices() {
        let s = TerminalStr::new("あb").unwrap();
        let indices: Vec<_> = TerminalCharIndices::new(s)
            .map(|(wl, c)| (wl.width.0, wl.len, c.as_str().as_str()))
            .collect();
        assert_eq!(vec![(0, 0, "あ"), (2, 3, "b")], indices);
    }

    #[test]
    fn test_char_indices_rev() {
        let s = TerminalStr::new("あb").unwrap();
        let indices: Vec<_> = TerminalCharIndices::new(s)
            .rev()
            .map(|(wl, c)| (wl.width.0, wl.len, c.as_str().as_str()))
            .collect();
        assert_eq!(vec![(2, 3, "b"), (0, 0, "あ")], indices);
    }
}
