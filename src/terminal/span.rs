use crate::terminal::style::Style;
use crate::terminal::terminal_str::string::TerminalString;
use crate::terminal::width::Width;

/// One run of terminal chars that share a style.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Span {
    pub(crate) style: Style,
    pub(crate) content: TerminalString,
}

impl Span {
    pub(crate) fn width(&self) -> Width {
        self.content.width()
    }
}
