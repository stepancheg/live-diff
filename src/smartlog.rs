use unicode_width::UnicodeWidthStr;

use crate::repo::digest::CommitDigest;
use crate::repo::sl::smartlog::SmartlogCommit;
use crate::repo::sl::smartlog::SmartlogRow;
use crate::repo::sl::smartlog::SmartlogRowKind;
use crate::repo::sl::smartlog::SmartlogView;
use crate::terminal::color::TerminalColor;
use crate::terminal::line::Line;
use crate::terminal::style::Style;
use crate::terminal::width::Width;
use crate::terminal_bg::TerminalBg;
use crate::util::time::ts_secs::TimestampsSecs;

/// Selected smartlog commit: full node digest and its index in the
/// view that selection was made against. [`SmartlogSelection::resolve`]
/// matches the digest first so a new commit on top does not move the
/// highlight; if that commit is gone, the index is used.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SmartlogSelection {
    pub(crate) digest: CommitDigest,
    pub(crate) index: usize,
}

impl SmartlogSelection {
    pub(crate) fn at(view: &SmartlogView, index: usize) -> Option<SmartlogSelection> {
        let commit = view.commits.get(index)?;
        Some(SmartlogSelection {
            digest: commit.node.clone(),
            index,
        })
    }

    /// Index in `view` of this selection. Digest wins; otherwise the
    /// stored index if it is still in range.
    pub(crate) fn resolve(&self, view: &SmartlogView) -> Option<usize> {
        for (i, commit) in view.commits.iter().enumerate() {
            if commit.node == self.digest {
                return Some(i);
            }
        }
        if self.index < view.commits.len() {
            return Some(self.index);
        }
        None
    }
}

/// Paint smartlog rows in the `sl sl` layout: graph, then hash, age,
/// author, bookmarks, and `#N`, with the description on the next row.
pub(crate) fn render_lines(
    view: &SmartlogView,
    selected: Option<&SmartlogSelection>,
    now_unix: TimestampsSecs,
    terminal_bg: TerminalBg,
    width: Width,
) -> Vec<Line> {
    let selected_index = match selected {
        Some(sel) => sel.resolve(view),
        None => None,
    };
    let mut lines = Vec::new();
    for row in &view.rows {
        let commit = match row.commit {
            Some(index) => view.commits.get(index),
            None => None,
        };
        let selected_row = match (selected_index, row.commit) {
            (Some(sel), Some(index)) => sel == index,
            _ => false,
        };
        let phase_style = match commit {
            Some(c) => phase_style(c, selected_row, terminal_bg),
            None => plain_row_style(selected_row, terminal_bg),
        };
        let mut line = Line::new();
        match row.kind {
            SmartlogRowKind::Header => {
                let Some(c) = commit else {
                    continue;
                };
                line.push_styled(&row.graph, phase_style);
                line.push_styled(&short_node(&c.node.0), phase_style);
                let age = format_age(c.date_unix, now_unix);
                let mut meta = Style::plain();
                meta.dim = true;
                apply_row_bg(&mut meta, selected_row, terminal_bg);
                if !age.is_empty() {
                    line.push_styled("  ", meta);
                    line.push_styled(&age, meta);
                }
                if !c.author.is_empty() {
                    line.push_styled("  ", meta);
                    line.push_styled(&c.author, meta);
                }
                let mut book = Style::plain();
                book.fg = Some(TerminalColor::Cyan);
                apply_row_bg(&mut book, selected_row, terminal_bg);
                for name in &c.bookmarks {
                    line.push_styled("  ", book);
                    line.push_styled(name, book);
                }
                if let Some(pr) = c.github_pr {
                    // Spaces stay unlinked and not underlined; only `#N` is.
                    line.push_styled("  ", book);
                    let mut pr_style = book;
                    pr_style.underline = true;
                    let label = format!("#{pr}");
                    match &view.forge {
                        Some(forge) => {
                            line.push_linked(&label, pr_style, &forge.pull_url(pr));
                        }
                        None => {
                            line.push_styled(&label, pr_style);
                        }
                    }
                }
            }
            SmartlogRowKind::Desc => {
                let Some(c) = commit else {
                    continue;
                };
                let text_col = UnicodeWidthStr::width(row_header_graph(view, c, row).as_str());
                let padded = pad_graph(&row.graph, text_col);
                let mut graph_style = Style::plain();
                apply_row_bg(&mut graph_style, selected_row, terminal_bg);
                line.push_styled(&padded, graph_style);
                let mut desc_style = Style::plain();
                if c.current {
                    desc_style.bold = true;
                }
                apply_row_bg(&mut desc_style, selected_row, terminal_bg);
                line.push_styled(&c.desc, desc_style);
            }
            SmartlogRowKind::Edge => {
                let mut graph_style = Style::plain();
                apply_row_bg(&mut graph_style, selected_row, terminal_bg);
                line.push_styled(&row.graph, graph_style);
            }
        }
        // Selected rows fill to the right edge, same as the diffstat.
        // Spaces cover the columns our width counts; fill_eol paints any
        // further screen cells (box-drawing glyphs are ambiguous-width).
        if selected_row {
            let cols = usize::try_from(width.0).unwrap_or(0);
            let used = usize::try_from(line.width().0).unwrap_or(0);
            if used < cols {
                let mut pad_style = Style::plain();
                apply_row_bg(&mut pad_style, true, terminal_bg);
                line.push_styled(&" ".repeat(cols - used), pad_style);
            }
            line.fill_eol = true;
        }
        lines.push(line);
    }
    lines
}

fn row_header_graph(view: &SmartlogView, commit: &SmartlogCommit, _row: &SmartlogRow) -> String {
    // The header row for this commit holds the graph width that text
    // is aligned to.
    for row in &view.rows {
        if row.kind != SmartlogRowKind::Header {
            continue;
        }
        if let Some(index) = row.commit {
            if let Some(c) = view.commits.get(index) {
                if c.node == commit.node {
                    return row.graph.clone();
                }
            }
        }
    }
    String::new()
}

fn pad_graph(graph: &str, text_col: usize) -> String {
    let width = UnicodeWidthStr::width(graph);
    if width >= text_col {
        return graph.to_owned();
    }
    let mut s = String::from(graph);
    let pad = text_col.saturating_sub(width);
    for _ in 0..pad {
        s.push(' ');
    }
    s
}

fn short_node(node: &str) -> String {
    // `sl sl` uses shortest(node, 10). Ten hex digits is that minimum
    // and is unique in any small fixture; live repos may need more, but
    // ten matches the usual smartlog hash.
    let mut s = String::new();
    for (i, ch) in node.chars().enumerate() {
        if i >= 10 {
            break;
        }
        s.push(ch);
    }
    s
}

fn format_age(ts: TimestampsSecs, now: TimestampsSecs) -> String {
    if ts.0 <= 0 {
        return String::new();
    }
    let age = now.saturating_sub(ts);
    if age.is_negative() {
        return String::from("in the future");
    }
    if age < jiff::SignedDuration::from_mins(1) {
        return plural(age.as_secs(), "second");
    }
    if age < jiff::SignedDuration::from_hours(1) {
        return plural(age.as_mins(), "minute");
    }
    if age < jiff::SignedDuration::from_hours(48) {
        return plural(age.as_hours(), "hour");
    }
    // 14 days of 24 hours. SignedDuration has no day unit.
    if age < jiff::SignedDuration::from_hours(14 * 24) {
        return plural(age.as_hours() / 24, "day");
    }
    // Fallback calendar date in UTC, YYYY-MM-DD.
    format_utc_date(ts.0)
}

fn plural(n: i64, unit: &str) -> String {
    if n == 1 {
        return format!("1 {unit} ago");
    }
    format!("{n} {unit}s ago")
}

fn format_utc_date(ts: i64) -> String {
    // Civil date from unix days, no chrono dependency.
    let days = ts.div_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Howard Hinnant civil_from_days.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m as u32, d as u32)
}

fn phase_style(commit: &SmartlogCommit, selected: bool, terminal_bg: TerminalBg) -> Style {
    let mut style = Style::plain();
    style.fg = Some(match commit.phase.as_str() {
        "public" => TerminalColor::Green,
        "secret" => TerminalColor::Magenta,
        _ => TerminalColor::Yellow,
    });
    if commit.current {
        style.bold = true;
    }
    apply_row_bg(&mut style, selected, terminal_bg);
    style
}

fn plain_row_style(selected: bool, terminal_bg: TerminalBg) -> Style {
    let mut style = Style::plain();
    apply_row_bg(&mut style, selected, terminal_bg);
    style
}

fn apply_row_bg(style: &mut Style, selected: bool, terminal_bg: TerminalBg) {
    if selected {
        style.bg = Some(terminal_bg.row_highlight());
    }
}

/// Row index of the header line for `commit`.
pub(crate) fn commit_header_row(view: &SmartlogView, commit: usize) -> usize {
    for (i, row) in view.rows.iter().enumerate() {
        if row.kind != SmartlogRowKind::Header {
            continue;
        }
        if row.commit == Some(commit) {
            return i;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use crate::repo::digest::CommitDigest;
    use crate::repo::forge::Forge;
    use crate::repo::sl::smartlog::SmartlogCommit;
    use crate::repo::sl::smartlog::SmartlogRow;
    use crate::repo::sl::smartlog::SmartlogRowKind;
    use crate::repo::sl::smartlog::SmartlogView;
    use crate::smartlog::SmartlogSelection;
    use crate::smartlog::format_age;
    use crate::smartlog::render_lines;
    use crate::terminal::width::Width;
    use crate::terminal_bg::TerminalBg;
    use crate::util::time::ts_secs::TimestampsSecs;

    #[test]
    fn test_smartlog_pr_label_is_linked_hash_number() {
        let view = SmartlogView {
            commits: vec![SmartlogCommit {
                node: CommitDigest(String::from("abc")),
                parents: Vec::new(),
                phase: String::from("draft"),
                bookmarks: Vec::new(),
                desc: String::from("d"),
                github_pr: Some(2032111),
                author: String::from("t"),
                date_unix: TimestampsSecs(0),
                current: false,
            }],
            rows: vec![SmartlogRow {
                commit: Some(0),
                graph: String::from("o  "),
                kind: SmartlogRowKind::Header,
            }],
            forge: Some(Forge::Origin {
                org: String::from("spacexsi"),
                name: String::from("xai"),
            }),
        };
        let lines = render_lines(&view, None, TimestampsSecs(0), TerminalBg::Dark, Width(80));
        let text = lines[0].plain_text();
        assert_eq!(true, text.contains("#2032111"));
        assert_eq!(false, text.contains("gh#"));
        let linked = lines[0]
            .spans
            .iter()
            .find(|s| s.link.is_some())
            .expect("pr span is a link");
        assert_eq!(
            Some(String::from(
                "https://cursor.com/codebase/spacexsi/xai/pull/2032111"
            )),
            linked.link.clone()
        );
        assert_eq!("#2032111", linked.content.as_ref().as_str());
    }

    #[test]
    fn test_smartlog_selected_rows_fill_width() {
        let view = SmartlogView {
            commits: vec![SmartlogCommit {
                node: CommitDigest(String::from("abc")),
                parents: Vec::new(),
                phase: String::from("draft"),
                bookmarks: Vec::new(),
                desc: String::from("d"),
                github_pr: None,
                author: String::from("t"),
                date_unix: TimestampsSecs(0),
                current: true,
            }],
            rows: vec![
                SmartlogRow {
                    commit: Some(0),
                    graph: String::from("@  "),
                    kind: SmartlogRowKind::Header,
                },
                SmartlogRow {
                    commit: Some(0),
                    graph: String::from("│  "),
                    kind: SmartlogRowKind::Desc,
                },
            ],
            forge: None,
        };
        let width = Width(40);
        let sel = SmartlogSelection {
            digest: CommitDigest(String::from("abc")),
            index: 0,
        };
        let lines = render_lines(
            &view,
            Some(&sel),
            TimestampsSecs(0),
            TerminalBg::Dark,
            width,
        );
        assert_eq!(2, lines.len());
        assert_eq!(width, lines[0].width());
        assert_eq!(width, lines[1].width());
        assert_eq!(true, lines[0].fill_eol);
        assert_eq!(true, lines[1].fill_eol);
        let unselected = render_lines(&view, None, TimestampsSecs(0), TerminalBg::Dark, width);
        assert_eq!(true, unselected[0].width().0 < width.0);
        assert_eq!(true, unselected[1].width().0 < width.0);
        assert_eq!(false, unselected[0].fill_eol);
        assert_eq!(false, unselected[1].fill_eol);
    }

    #[test]
    fn test_format_age_uses_signed_duration_units() {
        assert_eq!(
            "in the future",
            format_age(TimestampsSecs(100), TimestampsSecs(50))
        );
        assert_eq!(
            "5 seconds ago",
            format_age(TimestampsSecs(100), TimestampsSecs(105))
        );
        assert_eq!(
            "2 minutes ago",
            format_age(TimestampsSecs(100), TimestampsSecs(100 + 120))
        );
        assert_eq!(
            "3 hours ago",
            format_age(TimestampsSecs(100), TimestampsSecs(100 + 3 * 3600))
        );
        assert_eq!(
            "2 days ago",
            format_age(TimestampsSecs(100), TimestampsSecs(100 + 2 * 86400))
        );
        assert_eq!("", format_age(TimestampsSecs(0), TimestampsSecs(100)));
    }

    #[test]
    fn test_smartlog_selection_matches_digest_then_index() {
        let mut view = SmartlogView {
            commits: vec![
                SmartlogCommit {
                    node: CommitDigest(String::from("new")),
                    parents: Vec::new(),
                    phase: String::from("draft"),
                    bookmarks: Vec::new(),
                    desc: String::from("n"),
                    github_pr: None,
                    author: String::from("t"),
                    date_unix: TimestampsSecs(0),
                    current: true,
                },
                SmartlogCommit {
                    node: CommitDigest(String::from("old")),
                    parents: Vec::new(),
                    phase: String::from("draft"),
                    bookmarks: Vec::new(),
                    desc: String::from("o"),
                    github_pr: None,
                    author: String::from("t"),
                    date_unix: TimestampsSecs(0),
                    current: false,
                },
            ],
            rows: Vec::new(),
            forge: None,
        };
        // Selected "old" at index 0 before "new" landed on top.
        let sel = SmartlogSelection {
            digest: CommitDigest(String::from("old")),
            index: 0,
        };
        assert_eq!(Some(1), sel.resolve(&view));
        // Digest gone: fall back to the stored index.
        let gone = SmartlogSelection {
            digest: CommitDigest(String::from("missing")),
            index: 1,
        };
        assert_eq!(Some(1), gone.resolve(&view));
        view.commits.clear();
        assert_eq!(None, gone.resolve(&view));
    }
}
