use crate::terminal::span::Span;
use crate::terminal::style::Style;
use crate::terminal::terminal_str::string::TerminalString;
use crate::terminal::width::Width;

/// One row of styled spans. Not yet wrapped to the terminal width.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Line {
    pub(crate) spans: Vec<Span>,
    /// When true, the painter erases to the end of the screen row with
    /// the last span's background. That covers cells our column count
    /// misses (ambiguous-width glyphs that the terminal draws as one
    /// column).
    pub(crate) fill_eol: bool,
}

impl Line {
    pub(crate) fn new() -> Line {
        Line {
            spans: Vec::new(),
            fill_eol: false,
        }
    }

    /// One span of `text` in `style`. Controls are replaced with spaces.
    /// An empty `text` is a blank line with no spans.
    pub(crate) fn from_styled(text: &str, style: Style) -> Line {
        let mut line = Line::new();
        line.push_styled(text, style);
        line
    }

    /// Append a span. Empty `text` is ignored.
    pub(crate) fn push_styled(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        self.spans.push(Span {
            style,
            content: TerminalString::sanitize(text),
            link: None,
        });
    }

    /// Append a span that is an OSC 8 hyperlink to `url`.
    pub(crate) fn push_linked(&mut self, text: &str, style: Style, url: &str) {
        if text.is_empty() {
            return;
        }
        self.spans.push(Span {
            style,
            content: TerminalString::sanitize(text),
            link: Some(String::from(url)),
        });
    }

    pub(crate) fn width(&self) -> Width {
        self.spans.iter().map(|span| span.width()).sum()
    }

    pub(crate) fn plain_text(&self) -> String {
        let mut text = String::new();
        for span in &self.spans {
            text.push_str(span.content.as_ref().as_str());
        }
        text
    }

    /// Non-overlapping byte ranges of `re` in this line's plain text.
    /// Zero-width matches are skipped.
    pub(crate) fn match_ranges(&self, re: &regex::Regex) -> Vec<(usize, usize)> {
        let text = self.plain_text();
        let mut ranges = Vec::new();
        for m in re.find_iter(&text) {
            if m.start() < m.end() {
                ranges.push((m.start(), m.end()));
            }
        }
        ranges
    }
}

/// less case rules: `-I` always folds; `-i` folds only when `pattern` has no uppercase.
pub(crate) fn fold_case(pattern: &str, ignore_case: bool, ignore_case_always: bool) -> bool {
    ignore_case_always || (ignore_case && !pattern.chars().any(char::is_uppercase))
}

/// Compile `pattern` as a regex. `fold` enables case-insensitive matching.
pub(crate) fn compile_search(pattern: &str, fold: bool) -> Result<regex::Regex, regex::Error> {
    regex::RegexBuilder::new(pattern)
        .case_insensitive(fold)
        .build()
}
