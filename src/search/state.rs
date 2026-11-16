use std::cmp;

use crate::render::Renderer;
use crate::search::find::occurrences;
use crate::terminal::line::Line;
use crate::terminal::line::fold_case;

pub(crate) const PATTERN_NOT_FOUND: &str = "Pattern not found";
pub(crate) const NO_PREVIOUS_SEARCH: &str = "No previous search";
pub(crate) const INVALID_PATTERN: &str = "Invalid pattern";

/// Status line to show after a command-output refresh.
/// "Pattern not found" and the `-i`/`-I` case-mode message stay up
/// until the user issues a command. The case-mode message is rebuilt
/// from the fresh `[i/n]` count so it cannot keep a stale total.
pub(crate) fn status_after_output(
    current_status: Option<&str>,
    match_label: Option<String>,
) -> Option<String> {
    if let Some(status) = current_status {
        if let Some(mode_label) = case_mode_label_in(status) {
            return Some(with_match_label(mode_label, match_label.as_deref()));
        }
        if status == INVALID_PATTERN {
            return Some(INVALID_PATTERN.to_owned());
        }
        if status.starts_with(PATTERN_NOT_FOUND) {
            return Some(with_match_label(PATTERN_NOT_FOUND, match_label.as_deref()));
        }
    }
    match_label
}

pub(crate) fn is_case_mode_status(status: &str) -> bool {
    case_mode_label_in(status).is_some()
}

/// Mode label at the start of a case-mode status, with or without a
/// trailing ` [i/n]` count. "Ignore case" is a prefix of
/// "Ignore case always", so the longer label is tested first.
fn case_mode_label_in(status: &str) -> Option<&'static str> {
    let labels = [
        CaseMode::IgnoreCaseAlways.label(),
        CaseMode::IgnoreCase.label(),
        CaseMode::Sensitive.label(),
    ];
    for label in labels {
        if status == label {
            return Some(label);
        }
        if let Some(rest) = status.strip_prefix(label) {
            if rest.starts_with(' ') {
                return Some(label);
            }
        }
    }
    None
}

fn with_match_label(prefix: &str, match_label: Option<&str>) -> String {
    match match_label {
        Some(label) => {
            let mut msg = String::from(prefix);
            msg.push(' ');
            msg.push_str(label);
            msg
        }
        None => prefix.to_owned(),
    }
}

/// less `-i` / `-I` case handling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CaseMode {
    Sensitive,
    /// `-i`: ignore case when the pattern has no uppercase letters.
    IgnoreCase,
    /// `-I`: ignore case even when the pattern has uppercase letters.
    IgnoreCaseAlways,
}

impl CaseMode {
    /// Status text describing this `-i`/`-I` case mode.
    pub(crate) fn label(&self) -> &'static str {
        match *self {
            CaseMode::IgnoreCaseAlways => "Ignore case always",
            CaseMode::IgnoreCase => "Ignore case",
            CaseMode::Sensitive => "Case sensitive",
        }
    }

    /// Case-mode announcement, plus the current `[i/n]` count when there is one.
    pub(crate) fn status(&self, match_label: Option<&str>) -> String {
        with_match_label(self.label(), match_label)
    }
}

pub(crate) struct Search {
    pub(crate) pattern: String,
    pub(crate) forward: bool,
    case_mode: CaseMode,
    /// Selected hit: line index and byte offset in that line's plain text.
    /// `n`/`N` continue from the next occurrence.
    pub(crate) current: Option<(usize, usize)>,
}

/// Selected-hit position captured before a refresh replaces the lines.
#[derive(Clone, Copy)]
pub(crate) struct MatchAnchor {
    index: Option<usize>,
    line: usize,
    byte: usize,
}

impl Search {
    pub(crate) fn new() -> Search {
        Search {
            pattern: String::new(),
            forward: true,
            case_mode: CaseMode::Sensitive,
            current: None,
        }
    }

    pub(crate) fn case_mode(&self) -> CaseMode {
        self.case_mode
    }

    pub(crate) fn toggle_ignore_case(&mut self) {
        self.case_mode = match self.case_mode {
            CaseMode::IgnoreCase => CaseMode::Sensitive,
            CaseMode::Sensitive | CaseMode::IgnoreCaseAlways => CaseMode::IgnoreCase,
        };
    }

    pub(crate) fn toggle_ignore_case_always(&mut self) {
        self.case_mode = match self.case_mode {
            CaseMode::IgnoreCaseAlways => CaseMode::Sensitive,
            CaseMode::Sensitive | CaseMode::IgnoreCase => CaseMode::IgnoreCaseAlways,
        };
    }

    pub(crate) fn folds_pattern(&self) -> bool {
        let ignore_case = self.case_mode == CaseMode::IgnoreCase;
        let ignore_case_always = self.case_mode == CaseMode::IgnoreCaseAlways;
        fold_case(&self.pattern, ignore_case, ignore_case_always)
    }

    pub(crate) fn clear_match(&mut self) {
        self.current = None;
    }

    pub(crate) fn resume_from_history(&mut self, entries: &[String]) {
        if !self.pattern.is_empty() {
            return;
        }
        if let Some(last) = entries.last() {
            if !last.is_empty() {
                self.pattern = last.clone();
            }
        }
    }

    /// Remember which hit is selected, before the lines are replaced.
    pub(crate) fn anchor(&self, lines: &[Line]) -> Option<MatchAnchor> {
        let (line, byte) = self.current?;
        let index = occurrences(lines, &self.pattern, self.folds_pattern())
            .iter()
            .position(|occ| occ.line == line && occ.byte_start == byte);
        Some(MatchAnchor { index, line, byte })
    }

    /// Point `current` at a hit in the new lines. Prefer the same
    /// coordinates, then the same occurrence index.
    pub(crate) fn reattach(&mut self, lines: &[Line], anchor: Option<MatchAnchor>) {
        let Some(anchor) = anchor else {
            return;
        };
        let occs = occurrences(lines, &self.pattern, self.folds_pattern());
        if occs.is_empty() {
            self.current = None;
            return;
        }
        if let Some(occ) = occs
            .iter()
            .find(|occ| occ.line == anchor.line && occ.byte_start == anchor.byte)
        {
            self.current = Some((occ.line, occ.byte_start));
            return;
        }
        if let Some(index) = anchor.index {
            let occ = occs[cmp::min(index, occs.len() - 1)];
            self.current = Some((occ.line, occ.byte_start));
            return;
        }
        let occ = match occs.iter().find(|occ| occ.line >= anchor.line) {
            Some(occ) => *occ,
            None => occs[occs.len() - 1],
        };
        self.current = Some((occ.line, occ.byte_start));
    }

    /// If no hit is selected, select the first one on `top_line..=bottom_line`.
    /// Does not move the viewport.
    pub(crate) fn select_visible_match(
        &mut self,
        lines: &[Line],
        top_line: usize,
        bottom_line: usize,
    ) {
        if self.current.is_some() {
            return;
        }
        let occs = occurrences(lines, &self.pattern, self.folds_pattern());
        let Some(occ) = occs
            .iter()
            .find(|occ| occ.line >= top_line && occ.line <= bottom_line)
        else {
            return;
        };
        self.current = Some((occ.line, occ.byte_start));
    }

    /// Vim-style `[i/n]` for the selected hit, if there is one.
    pub(crate) fn match_label(&self, lines: &[Line]) -> Option<String> {
        let (line, byte) = self.current?;
        let occs = occurrences(lines, &self.pattern, self.folds_pattern());
        let index = occs
            .iter()
            .position(|occ| occ.line == line && occ.byte_start == byte)?;
        Some(format!("[{}/{}]", index + 1, occs.len()))
    }

    pub(crate) fn apply_highlight(&self, renderer: &mut Renderer) {
        if self.pattern.is_empty() {
            renderer.set_highlight(None, None, false);
        } else {
            renderer.set_highlight(
                Some(self.pattern.clone()),
                self.current,
                self.folds_pattern(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU16;

    use crate::height::Height;
    use crate::render::Renderer;
    use crate::search::find::SearchOutcome;
    use crate::search::find::search_next;
    use crate::search::find::submit_prompt;
    use crate::search::state::CaseMode;
    use crate::search::state::Search;
    use crate::search::state::status_after_output;
    use crate::size::Size;
    use crate::terminal::parse::parse_command_output;
    use crate::terminal::width::Width;
    use crate::terminal_bg::TerminalBg;

    fn size(width: u32, height: u32) -> Size {
        Size {
            width: Width(width),
            height: Height(height),
        }
    }

    #[test]
    fn test_ignore_case_matches_unless_pattern_has_uppercase() {
        let size = size(10, 6);
        let mut renderer = Renderer::new(TerminalBg::Dark, size);
        renderer.set_lines(parse_command_output(b"FOO\nfoo\nFoo\nbar\n"));
        let mut search = Search::new();
        search.toggle_ignore_case();
        assert_eq!(CaseMode::IgnoreCase, search.case_mode());
        assert_eq!(
            SearchOutcome::Found,
            submit_prompt(&mut renderer, &mut search, true, "foo".to_owned())
        );
        assert_eq!(
            Some("[1/3]".to_owned()),
            search.match_label(renderer.lines())
        );
        // less -i: a pattern with uppercase is matched case-sensitively.
        assert_eq!(
            SearchOutcome::Found,
            submit_prompt(&mut renderer, &mut search, true, "Foo".to_owned())
        );
        assert_eq!(
            Some("[1/1]".to_owned()),
            search.match_label(renderer.lines())
        );
        // less -I: ignore case even when the pattern has uppercase.
        search.toggle_ignore_case_always();
        assert_eq!(CaseMode::IgnoreCaseAlways, search.case_mode());
        assert_eq!(
            SearchOutcome::Found,
            submit_prompt(&mut renderer, &mut search, true, "Foo".to_owned())
        );
        assert_eq!(
            Some("[1/3]".to_owned()),
            search.match_label(renderer.lines())
        );
        assert_eq!("Ignore case always", search.case_mode().label());
    }

    #[test]
    fn test_toggle_ignore_case_refreshes_hits() {
        let size = size(10, 6);
        let mut renderer = Renderer::new(TerminalBg::Dark, size);
        renderer.set_lines(parse_command_output(b"FOO\nfoo\nFoo\nbar\n"));
        let mut search = Search::new();
        assert_eq!(
            SearchOutcome::Found,
            submit_prompt(&mut renderer, &mut search, true, "foo".to_owned())
        );
        assert_eq!(
            Some("[1/1]".to_owned()),
            search.match_label(renderer.lines())
        );
        let anchor = search.anchor(renderer.lines());
        search.toggle_ignore_case();
        search.reattach(renderer.lines(), anchor);
        search.apply_highlight(&mut renderer);
        assert_eq!(
            Some("[2/3]".to_owned()),
            search.match_label(renderer.lines())
        );
        assert_eq!(
            "Ignore case [2/3]",
            search
                .case_mode()
                .status(search.match_label(renderer.lines()).as_deref())
        );
        let mut buf = Vec::new();
        renderer.redraw(&mut buf).unwrap();
        let mut parser =
            vt100::Parser::new(NonZeroU16::new(6).unwrap(), NonZeroU16::new(10).unwrap(), 0);
        parser.process(&buf);
        // "FOO" on line 0 is now a hit.
        let cell = parser.screen().cell(0, 0).unwrap();
        assert_eq!(false, cell.bgcolor() == vt100::Color::Default);
    }

    #[test]
    fn test_reattach_keeps_current_across_refresh() {
        let size = size(20, 8);
        let mut renderer = Renderer::new(TerminalBg::Dark, size);
        renderer.set_lines(parse_command_output(b"foo foo\nbar\nfoo\nx\nx\nx\nx\nx\n"));
        let mut search = Search::new();
        submit_prompt(&mut renderer, &mut search, true, "foo".to_owned());
        search_next(&mut renderer, &mut search, false);
        assert_eq!(
            Some("[2/3]".to_owned()),
            search.match_label(renderer.lines())
        );
        let anchor = search.anchor(renderer.lines());
        // Same text, as a refresh that does not move the hit.
        let again = parse_command_output(b"foo foo\nbar\nfoo\nx\nx\nx\nx\nx\n");
        search.reattach(&again, anchor);
        assert_eq!(Some("[2/3]".to_owned()), search.match_label(&again));
        // A line inserted above shifts coordinates; keep the same index.
        let shifted = parse_command_output(b"zz\nfoo foo\nbar\nfoo\nx\nx\nx\nx\nx\n");
        search.reattach(&shifted, anchor);
        assert_eq!(Some("[2/3]".to_owned()), search.match_label(&shifted));
    }

    #[test]
    fn test_pattern_not_found_survives_output_refresh() {
        assert_eq!(
            Some("Pattern not found".to_owned()),
            status_after_output(Some("Pattern not found"), None)
        );
        assert_eq!(
            Some("Pattern not found [2/3]".to_owned()),
            status_after_output(Some("Pattern not found [1/3]"), Some("[2/3]".to_owned()))
        );
        assert_eq!(
            Some("[2/3]".to_owned()),
            status_after_output(Some("[1/3]"), Some("[2/3]".to_owned()))
        );
        assert_eq!(None, status_after_output(None, None));
        assert_eq!(
            Some("Ignore case [2/3]".to_owned()),
            status_after_output(Some("Ignore case"), Some("[2/3]".to_owned()))
        );
        assert_eq!(
            Some("Ignore case always".to_owned()),
            status_after_output(Some("Ignore case always"), None)
        );
        assert_eq!(
            Some("Case sensitive [1/1]".to_owned()),
            status_after_output(Some("Case sensitive"), Some("[1/1]".to_owned()))
        );
        // A stale count on the case-mode line is replaced, not kept.
        // "Ignore case always" must not be read as "Ignore case" + " always".
        assert_eq!(
            Some("Ignore case [4/5]".to_owned()),
            status_after_output(Some("Ignore case [2/3]"), Some("[4/5]".to_owned()))
        );
        assert_eq!(
            Some("Ignore case always [1/2]".to_owned()),
            status_after_output(Some("Ignore case always [9/9]"), Some("[1/2]".to_owned()))
        );
        assert_eq!(
            Some("Ignore case".to_owned()),
            status_after_output(Some("Ignore case [2/3]"), None)
        );
    }

    #[test]
    fn test_toggle_ignore_case_selects_visible_hit() {
        let size = size(10, 6);
        let mut renderer = Renderer::new(TerminalBg::Dark, size);
        // Height 6 → 5 body rows. 12 lines, so the last page starts at line 7
        // and does not include the line-0 "foo".
        renderer.set_lines(parse_command_output(
            b"foo\nx\nx\nx\nx\nx\nx\nx\nx\nx\nFOO\nfoo\n",
        ));
        let mut search = Search::new();
        assert_eq!(
            SearchOutcome::Found,
            submit_prompt(&mut renderer, &mut search, true, "foo".to_owned())
        );
        assert_eq!(
            Some("[1/2]".to_owned()),
            search.match_label(renderer.lines())
        );
        // A move drops the selected hit. -i must still adopt a visible hit
        // under the new case mode, without scrolling.
        renderer.scroll_bottom();
        search.clear_match();
        let top = renderer.top_line_index();
        assert_eq!(7, top);
        search.toggle_ignore_case();
        search.reattach(renderer.lines(), None);
        let bottom = renderer.bottom_line_index();
        search.select_visible_match(renderer.lines(), top, bottom);
        search.apply_highlight(&mut renderer);
        assert_eq!(7, renderer.top_line_index());
        // Visible ignore-case hits are line 10 "FOO" and line 11 "foo".
        // All hits are foo, FOO, foo → the on-screen FOO is [2/3].
        assert_eq!(
            Some("[2/3]".to_owned()),
            search.match_label(renderer.lines())
        );
        assert_eq!(
            "Ignore case [2/3]",
            search
                .case_mode()
                .status(search.match_label(renderer.lines()).as_deref())
        );
    }
}
