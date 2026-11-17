use std::process::Command;
use std::str;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use unicode_width::UnicodeWidthStr;

use crate::terminal::color::TerminalColor;
use crate::terminal::line::Line;
use crate::terminal::style::Style;
use crate::terminal_bg::TerminalBg;

/// Null parent hash emitted by `p1node` / `p2node` when that parent
/// does not exist.
const NULL_NODE: &str = "0000000000000000000000000000000000000000";

/// One commit from [`sl_smartlog_command`], enough to rebuild the
/// parent DAG `sl sl` draws.
pub(crate) struct SmartlogCommit {
    pub(crate) node: String,
    /// Present parents only, first parent then second.
    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) parents: Vec<String>,
    pub(crate) phase: String,
    pub(crate) bookmarks: Vec<String>,
    pub(crate) desc: String,
    /// GitHub pull request number when the commit is tied to one.
    pub(crate) github_pr: Option<u64>,
    pub(crate) author: String,
    /// Commit time as unix seconds.
    pub(crate) date_unix: i64,
    /// True when this commit is the working-copy parent (graph `@`).
    pub(crate) current: bool,
}

/// One painted row of a smartlog, with Sapling's graph prefix kept
/// verbatim from `sl smartlog`.
pub(crate) struct SmartlogRow {
    /// Index into [`SmartlogView::commits`] for the header row of that
    /// commit. Desc rows carry the same index. Pure edge rows are `None`.
    pub(crate) commit: Option<usize>,
    pub(crate) graph: String,
    pub(crate) kind: SmartlogRowKind,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum SmartlogRowKind {
    Header,
    Desc,
    Edge,
}

pub(crate) struct SmartlogView {
    pub(crate) commits: Vec<SmartlogCommit>,
    pub(crate) rows: Vec<SmartlogRow>,
    /// Where `#N` pull-request links should point.
    pub(crate) forge: Option<Forge>,
}

/// Host that owns pull requests for this checkout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Forge {
    GitHub { owner: String, repo: String },
    Origin { org: String, name: String },
}

impl Forge {
    pub(crate) fn pull_url(&self, number: u64) -> String {
        match self {
            Forge::GitHub { owner, repo } => {
                format!("https://github.com/{owner}/{repo}/pull/{number}")
            }
            Forge::Origin { org, name } => {
                format!("https://cursor.com/codebase/{org}/{name}/pull/{number}")
            }
        }
    }
}

/// `sl smartlog` with a JSON commit record and the same three-line
/// template shape as `sl sl` (`header`, `desc`, separator), so the
/// graph glyphs match Sapling's.
pub(crate) fn sl_smartlog_command() -> Vec<String> {
    // `{sl}` emits header, description, and a separator newline; the
    // two extra newlines after the JSON line recreate that row shape.
    // Graph columns are drawn by smartlog and prefixed on every line.
    let mut cmd = Vec::new();
    cmd.push(String::from("sl"));
    cmd.push(String::from("--color=never"));
    cmd.push(String::from("--pager=never"));
    cmd.push(String::from("smartlog"));
    cmd.push(String::from("-T"));
    cmd.push(String::from(
        "{dict(node=node, p1=p1node, p2=p2node, phase=phase, bookmarks=bookmarks, desc=desc|firstline, pr=github_pull_request_number, user=author|emailuser, date=date|hgdate, cur=ifcontains(rev, revset(\".\"), \"1\", \"0\"))|json}\n\n\n",
    ));
    cmd
}

/// Run [`sl_smartlog_command`] in the current directory.
pub(crate) fn fetch_smartlog() -> anyhow::Result<SmartlogView> {
    let argv = sl_smartlog_command();
    let Some((program, args)) = argv.split_first() else {
        anyhow::bail!("empty smartlog command");
    };
    let output = Command::new(program)
        .args(args)
        .env_remove("FORCE_COLOR")
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("sl smartlog failed: {stderr}");
    }
    let mut view = parse_smartlog_view(&output.stdout)?;
    view.forge = detect_forge();
    Ok(view)
}

/// Pick Origin when a remote is `origin.cursor.com`, otherwise GitHub.
fn detect_forge() -> Option<Forge> {
    let mut github = None;
    for url in remote_urls() {
        if let Some(forge) = forge_from_remote_url(&url) {
            match forge {
                Forge::Origin { .. } => return Some(forge),
                Forge::GitHub { .. } => {
                    if github.is_none() {
                        github = Some(forge);
                    }
                }
            }
        }
    }
    github
}

fn remote_urls() -> Vec<String> {
    let mut urls = Vec::new();
    if let Ok(out) = Command::new("git").args(["remote", "-v"]).output() {
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout);
            for line in text.lines() {
                // "name URL (fetch)"
                let mut parts = line.split_whitespace();
                let _name = parts.next();
                if let Some(url) = parts.next() {
                    urls.push(url.to_owned());
                }
            }
        }
    }
    urls
}

/// Parse a git remote into a forge. `.git` suffix is ignored.
pub(crate) fn forge_from_remote_url(url: &str) -> Option<Forge> {
    let url = url.trim();
    // Drop userinfo so `https://user@host/path` still parses.
    let without_user = match url.find("://") {
        Some(scheme_end) => {
            let rest = &url[scheme_end + 3..];
            match rest.find('@') {
                Some(at) => {
                    let mut s = String::from(&url[..scheme_end + 3]);
                    s.push_str(&rest[at + 1..]);
                    s
                }
                None => url.to_owned(),
            }
        }
        None => url.to_owned(),
    };
    // scp-like: git@host:org/name.git
    if let Some(at) = without_user.find('@') {
        if !without_user.contains("://") {
            let after = &without_user[at + 1..];
            if let Some(colon) = after.find(':') {
                let host = &after[..colon];
                let path = &after[colon + 1..];
                return forge_from_host_path(host, path);
            }
        }
    }
    if let Some(scheme_end) = without_user.find("://") {
        let rest = &without_user[scheme_end + 3..];
        if !rest.contains('/') {
            return None;
        }
        // rest is host/path...
        let slash = rest.find('/')?;
        let host = &rest[..slash];
        let path = &rest[slash + 1..];
        // host may include :port
        let host = match host.find(':') {
            Some(i) => &host[..i],
            None => host,
        };
        return forge_from_host_path(host, path);
    }
    None
}

fn forge_from_host_path(host: &str, path: &str) -> Option<Forge> {
    let path = path.trim_end_matches('/').trim_end_matches(".git");
    let mut parts = path.split('/');
    let a = parts.next()?;
    let b = parts.next()?;
    if a.is_empty() || b.is_empty() {
        return None;
    }
    // Ignore extra path segments (e.g. tree/branch); org/name is enough
    // when the first two segments are the repo.
    if host == "origin.cursor.com" {
        return Some(Forge::Origin {
            org: a.to_owned(),
            name: b.to_owned(),
        });
    }
    if host == "github.com" {
        return Some(Forge::GitHub {
            owner: a.to_owned(),
            repo: b.to_owned(),
        });
    }
    None
}

/// Parse NDJSON from [`sl_smartlog_command`]. Graph prefixes and
/// edge-only lines are skipped; only the commit records are returned.
#[cfg_attr(not(test), expect(dead_code))]
pub(crate) fn parse_smartlog(bytes: &[u8]) -> anyhow::Result<Vec<SmartlogCommit>> {
    Ok(parse_smartlog_view(bytes)?.commits)
}

/// Parse smartlog stdout into commits plus the graph rows to paint.
pub(crate) fn parse_smartlog_view(bytes: &[u8]) -> anyhow::Result<SmartlogView> {
    let text = str::from_utf8(bytes)?;
    let mut commits = Vec::new();
    let mut rows = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let Some(json_at) = line.find('{') else {
            continue;
        };
        let graph = line[..json_at].to_owned();
        let commit = parse_smartlog_line(&line[json_at..])?;
        let index = commits.len();
        commits.push(commit);
        rows.push(SmartlogRow {
            commit: Some(index),
            graph,
            kind: SmartlogRowKind::Header,
        });
        // Template newlines after the JSON line are graph-only rows.
        // The first carries the description; later ones are separators.
        let mut saw_desc = false;
        while let Some(cont) = lines.peek() {
            if cont.contains('{') {
                break;
            }
            let cont = lines.next().unwrap_or("");
            if !saw_desc {
                rows.push(SmartlogRow {
                    commit: Some(index),
                    graph: cont.to_owned(),
                    kind: SmartlogRowKind::Desc,
                });
                saw_desc = true;
            } else {
                rows.push(SmartlogRow {
                    commit: None,
                    graph: cont.to_owned(),
                    kind: SmartlogRowKind::Edge,
                });
            }
        }
    }
    Ok(SmartlogView {
        commits,
        rows,
        forge: None,
    })
}

/// Wire shape of one `sl log` JSON object. Null parents are the
/// all-zero hash; [`SmartlogCommit`] drops those.
#[derive(serde::Deserialize)]
struct RawCommit {
    node: String,
    p1: String,
    p2: String,
    phase: String,
    bookmarks: Vec<String>,
    desc: String,
    /// Absent PR is JSON null or `""`, depending on the repo.
    pr: serde_json::Value,
    #[serde(default)]
    user: String,
    #[serde(default)]
    date: String,
    #[serde(default)]
    cur: String,
}

fn parse_smartlog_line(line: &str) -> anyhow::Result<SmartlogCommit> {
    let raw: RawCommit = serde_json::from_str(line)?;
    let mut parents = Vec::new();
    if raw.p1 != NULL_NODE {
        parents.push(raw.p1);
    }
    if raw.p2 != NULL_NODE {
        parents.push(raw.p2);
    }
    Ok(SmartlogCommit {
        node: raw.node,
        parents,
        phase: raw.phase,
        bookmarks: raw.bookmarks,
        desc: raw.desc,
        github_pr: github_pr_number(&raw.pr)?,
        author: raw.user,
        date_unix: hgdate_unix(&raw.date),
        current: raw.cur == "1",
    })
}

fn hgdate_unix(date: &str) -> i64 {
    let Some(secs) = date.split_whitespace().next() else {
        return 0;
    };
    secs.parse::<i64>().unwrap_or(0)
}

fn github_pr_number(value: &serde_json::Value) -> anyhow::Result<Option<u64>> {
    match value {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::String(text) => {
            if text.is_empty() {
                return Ok(None);
            }
            let number = text.parse::<u64>()?;
            Ok(Some(number))
        }
        serde_json::Value::Number(number) => {
            let Some(value) = number.as_u64() else {
                anyhow::bail!("github pr number is not a u64");
            };
            Ok(Some(value))
        }
        _ => anyhow::bail!("github pr number has unexpected json type"),
    }
}

/// Paint smartlog rows in the `sl sl` layout: graph, then hash, age,
/// author, bookmarks, and `gh#N`, with the description on the next row.
pub(crate) fn smartlog_lines(
    view: &SmartlogView,
    selected: Option<usize>,
    now_unix: i64,
    terminal_bg: TerminalBg,
) -> Vec<Line> {
    let mut lines = Vec::new();
    for row in &view.rows {
        let commit = match row.commit {
            Some(index) => view.commits.get(index),
            None => None,
        };
        let selected_row = match (selected, row.commit) {
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
                line.push_styled(&short_node(&c.node), phase_style);
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

fn format_age(ts: i64, now: i64) -> String {
    if ts <= 0 {
        return String::new();
    }
    if now < ts {
        return String::from("in the future");
    }
    let secs = now - ts;
    if secs < 60 {
        return plural(secs, "second");
    }
    if secs < 3600 {
        return plural(secs / 60, "minute");
    }
    if secs < 48 * 3600 {
        return plural(secs / 3600, "hour");
    }
    if secs < 14 * 86400 {
        return plural(secs / 86400, "day");
    }
    // Fallback calendar date in UTC, YYYY-MM-DD.
    format_utc_date(ts)
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

/// Index of the current commit (`@`), if it is in the view.
pub(crate) fn current_commit_index(view: &SmartlogView) -> Option<usize> {
    view.commits.iter().position(|c| c.current)
}

/// Index of the commit named by `rev`: full hash, unique hash prefix,
/// or `.` / `HEAD` for the working-copy parent.
pub(crate) fn find_commit_index(view: &SmartlogView, rev: &str) -> Option<usize> {
    if rev == "." || rev == "HEAD" {
        return current_commit_index(view);
    }
    if let Some(i) = view.commits.iter().position(|c| c.node == rev) {
        return Some(i);
    }
    let mut found = None;
    for (i, commit) in view.commits.iter().enumerate() {
        if commit.node.starts_with(rev) {
            if found.is_some() {
                return None;
            }
            found = Some(i);
        }
    }
    found
}

pub(crate) fn unix_now() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use crate::repo::sl_smartlog::parse_smartlog;
    use crate::repo::sl_smartlog::sl_smartlog_command;
    use crate::repo::test_support::TestRepo;

    #[test]
    fn test_parse_smartlog_json_lines() {
        let bytes = br#"{"bookmarks": ["main"], "desc": "say \"hi\"", "node": "aaa", "p1": "bbb", "p2": "0000000000000000000000000000000000000000", "phase": "draft", "pr": 12}
{"bookmarks": [], "desc": "merge", "node": "ccc", "p1": "aaa", "p2": "ddd", "phase": "draft", "pr": null}
"#;
        let commits = parse_smartlog(bytes).unwrap();
        assert_eq!(2, commits.len());
        assert_eq!("aaa", commits[0].node);
        assert_eq!(vec![String::from("bbb")], commits[0].parents);
        assert_eq!(vec![String::from("main")], commits[0].bookmarks);
        assert_eq!("say \"hi\"", commits[0].desc);
        assert_eq!("draft", commits[0].phase);
        assert_eq!(Some(12), commits[0].github_pr);
        assert_eq!(None, commits[1].github_pr);
        assert_eq!(
            vec![String::from("aaa"), String::from("ddd")],
            commits[1].parents
        );
        assert_eq!("merge", commits[1].desc);
    }

    #[test]
    fn test_parse_smartlog_skips_blank_lines() {
        let commits = parse_smartlog(b"\n").unwrap();
        assert_eq!(0, commits.len());
    }

    #[test]
    fn test_sl_smartlog_query_builds_tree() {
        // Two drafts forked from one root. Parents must point at root,
        // and root has no parent:
        //
        //   o  right
        //   |
        //   | o  left
        //   |/
        //   o  root
        let repo = TestRepo::init_sl();
        repo.write("a.txt", "root\n");
        let root = sl_commit(&repo, "root");
        repo.write("b.txt", "left\n");
        let left = sl_commit(&repo, "left\n\nbody");
        repo.ok("sl", &["goto", "-q", &root]);
        repo.write("c.txt", "right\n");
        let right = sl_commit(&repo, "right");

        let bytes = repo.run_ok(&sl_smartlog_command());
        let view = crate::repo::sl_smartlog::parse_smartlog_view(&bytes).unwrap();
        let painted = crate::repo::sl_smartlog::smartlog_lines(
            &view,
            None,
            crate::repo::sl_smartlog::unix_now(),
            crate::terminal_bg::TerminalBg::Dark,
        );
        let text: String = painted
            .iter()
            .map(|line| line.plain_text())
            .collect::<Vec<String>>()
            .join("\n");
        // Same fork Sapling draws: the side commit joins with ├─╯.
        assert_eq!(true, text.contains("├─╯"));
        assert_eq!(true, text.contains("left"));
        assert_eq!(true, text.contains("right"));
        assert_eq!(true, text.contains("root"));
        let commits = parse_smartlog(&bytes).unwrap();
        let mut by_desc: Vec<(String, String, Vec<String>)> = commits
            .iter()
            .map(|c| (c.desc.clone(), c.node.clone(), c.parents.clone()))
            .collect();
        by_desc.sort();
        assert_eq!(
            vec![
                (String::from("left"), left, vec![root.clone()]),
                (String::from("right"), right, vec![root.clone()]),
                (String::from("root"), root, Vec::new()),
            ],
            by_desc
        );
    }

    #[test]
    fn test_find_commit_index_by_hash_prefix() {
        use crate::repo::sl_smartlog::SmartlogCommit;
        use crate::repo::sl_smartlog::SmartlogRow;
        use crate::repo::sl_smartlog::SmartlogRowKind;
        use crate::repo::sl_smartlog::SmartlogView;
        use crate::repo::sl_smartlog::find_commit_index;
        let commit = |node: &str, current: bool| SmartlogCommit {
            node: String::from(node),
            parents: Vec::new(),
            phase: String::from("draft"),
            bookmarks: Vec::new(),
            desc: String::from("d"),
            github_pr: None,
            author: String::new(),
            date_unix: 0,
            current,
        };
        let view = SmartlogView {
            commits: vec![
                commit("aaa1111111111111111111111111111111111111", false),
                commit("bbb2222222222222222222222222222222222222", true),
            ],
            rows: vec![SmartlogRow {
                commit: Some(0),
                graph: String::new(),
                kind: SmartlogRowKind::Header,
            }],
            forge: None,
        };
        assert_eq!(
            Some(1),
            find_commit_index(&view, "bbb2222222222222222222222222222222222222")
        );
        assert_eq!(Some(0), find_commit_index(&view, "aaa111"));
        assert_eq!(Some(1), find_commit_index(&view, "."));
        assert_eq!(Some(1), find_commit_index(&view, "HEAD"));
        // Ambiguous one-character prefix.
        assert_eq!(None, find_commit_index(&view, ""));
    }

    #[test]
    fn test_forge_from_remote_url() {
        use crate::repo::sl_smartlog::Forge;
        use crate::repo::sl_smartlog::forge_from_remote_url;
        assert_eq!(
            Some(Forge::GitHub {
                owner: String::from("stepancheg"),
                repo: String::from("live-diff"),
            }),
            forge_from_remote_url("https://github.com/stepancheg/live-diff.git")
        );
        assert_eq!(
            Some(Forge::GitHub {
                owner: String::from("stepancheg"),
                repo: String::from("live-diff"),
            }),
            forge_from_remote_url("git@github.com:stepancheg/live-diff.git")
        );
        assert_eq!(
            Some(Forge::Origin {
                org: String::from("spacexsi"),
                name: String::from("xai"),
            }),
            forge_from_remote_url("https://origin.cursor.com/spacexsi/xai.git")
        );
        assert_eq!(
            "https://cursor.com/codebase/spacexsi/xai/pull/2032111",
            Forge::Origin {
                org: String::from("spacexsi"),
                name: String::from("xai"),
            }
            .pull_url(2032111)
        );
        assert_eq!(
            "https://github.com/stepancheg/live-diff/pull/12",
            Forge::GitHub {
                owner: String::from("stepancheg"),
                repo: String::from("live-diff"),
            }
            .pull_url(12)
        );
    }

    #[test]
    fn test_smartlog_pr_label_is_linked_hash_number() {
        use crate::repo::sl_smartlog::Forge;
        use crate::repo::sl_smartlog::SmartlogCommit;
        use crate::repo::sl_smartlog::SmartlogRow;
        use crate::repo::sl_smartlog::SmartlogRowKind;
        use crate::repo::sl_smartlog::SmartlogView;
        use crate::repo::sl_smartlog::smartlog_lines;
        use crate::terminal_bg::TerminalBg;
        let view = SmartlogView {
            commits: vec![SmartlogCommit {
                node: String::from("abc"),
                parents: Vec::new(),
                phase: String::from("draft"),
                bookmarks: Vec::new(),
                desc: String::from("d"),
                github_pr: Some(2032111),
                author: String::from("t"),
                date_unix: 0,
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
        let lines = smartlog_lines(&view, None, 0, TerminalBg::Dark);
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

    fn sl_commit(repo: &TestRepo, message: &str) -> String {
        repo.ok("sl", &["addremove"]);
        repo.ok("sl", &["commit", "-m", message]);
        let hash = repo.ok("sl", &["log", "-r", ".", "-T", "{node}\n"]);
        hash.trim().to_owned()
    }
}
