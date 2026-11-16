use crate::terminal::span::Span;
use crate::terminal::width::Width;

/// One row of styled spans. Not yet wrapped to the terminal width.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Line {
    pub(crate) spans: Vec<Span>,
}

impl Line {
    pub(crate) fn new() -> Line {
        Line { spans: Vec::new() }
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
