use crate::terminal::style::Style;
use crate::terminal::terminal_str::string::TerminalString;
use crate::terminal::width::Width;

/// One run of terminal chars that share a style.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Span {
    pub(crate) style: Style,
    pub(crate) content: TerminalString,
    /// OSC 8 hyperlink target. `None` means the text is not a link.
    pub(crate) link: Option<String>,
}

impl Span {
    pub(crate) fn width(&self) -> Width {
        self.content.width()
    }
}
