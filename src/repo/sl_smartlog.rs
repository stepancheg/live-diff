use std::process::Command;
use std::str;

use crate::repo::digest::CommitDigest;
use crate::repo::forge::Forge;
use crate::repo::forge::detect_forge;
use crate::util::time::ts_secs::TimestampsSecs;

/// Null parent hash emitted by `p1node` / `p2node` when that parent
/// does not exist.
const NULL_NODE: &str = "0000000000000000000000000000000000000000";

/// One commit from [`sl_smartlog_command`], enough to rebuild the
/// parent DAG `sl sl` draws.
pub(crate) struct SmartlogCommit {
    pub(crate) node: CommitDigest,
    /// Present parents only, first parent then second.
    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) parents: Vec<CommitDigest>,
    pub(crate) phase: String,
    pub(crate) bookmarks: Vec<String>,
    pub(crate) desc: String,
    /// GitHub pull request number when the commit is tied to one.
    pub(crate) github_pr: Option<u64>,
    pub(crate) author: String,
    /// Commit time as unix seconds.
    pub(crate) date_unix: TimestampsSecs,
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

/// `sl smartlog` with a JSON commit record and the same three-line
/// template shape as `sl sl` (`header`, `desc`, separator), so the
/// graph glyphs match Sapling's.
fn sl_smartlog_command() -> Vec<String> {
    // `{sl}` emits header, description, and a separator newline; the
    // two extra newlines after the JSON line recreate that row shape.
    // Graph columns are drawn by smartlog and prefixed on every line.
    let mut cmd = Vec::new();
    cmd.push(String::from("sl"));
    cmd.push(String::from("--color=never"));
    cmd.push(String::from("--pager=never"));
    // Box-drawing graph (`│`, `├─╯`), not the ASCII `|` `/` renderer.
    // Overrides a user or repo `experimental.graph.renderer=ascii`.
    cmd.push(String::from("--config"));
    cmd.push(String::from("experimental.graph.renderer=lines"));
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

/// Parse NDJSON from [`sl_smartlog_command`]. Graph prefixes and
/// edge-only lines are skipped; only the commit records are returned.
#[cfg_attr(not(test), expect(dead_code))]
fn parse_smartlog(bytes: &[u8]) -> anyhow::Result<Vec<SmartlogCommit>> {
    Ok(parse_smartlog_view(bytes)?.commits)
}

/// Parse smartlog stdout into commits plus the graph rows to paint.
fn parse_smartlog_view(bytes: &[u8]) -> anyhow::Result<SmartlogView> {
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
        parents.push(CommitDigest(raw.p1));
    }
    if raw.p2 != NULL_NODE {
        parents.push(CommitDigest(raw.p2));
    }
    Ok(SmartlogCommit {
        node: CommitDigest(raw.node),
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

fn hgdate_unix(date: &str) -> TimestampsSecs {
    let Some(secs) = date.split_whitespace().next() else {
        return TimestampsSecs(0);
    };
    TimestampsSecs(secs.parse::<i64>().unwrap_or(0))
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
    if let Some(i) = view.commits.iter().position(|c| c.node.0 == rev) {
        return Some(i);
    }
    let mut found = None;
    for (i, commit) in view.commits.iter().enumerate() {
        if commit.node.0.starts_with(rev) {
            if found.is_some() {
                return None;
            }
            found = Some(i);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use crate::repo::digest::CommitDigest;
    use crate::repo::sl_smartlog::parse_smartlog;
    use crate::repo::sl_smartlog::sl_smartlog_command;
    use crate::repo::test_support::TestRepo;
    use crate::util::time::ts_secs::TimestampsSecs;

    #[test]
    fn test_parse_smartlog_json_lines() {
        let bytes = br#"{"bookmarks": ["main"], "desc": "say \"hi\"", "node": "aaa", "p1": "bbb", "p2": "0000000000000000000000000000000000000000", "phase": "draft", "pr": 12}
{"bookmarks": [], "desc": "merge", "node": "ccc", "p1": "aaa", "p2": "ddd", "phase": "draft", "pr": null}
"#;
        let commits = parse_smartlog(bytes).unwrap();
        assert_eq!(2, commits.len());
        assert_eq!(CommitDigest(String::from("aaa")), commits[0].node);
        assert_eq!(vec![CommitDigest(String::from("bbb"))], commits[0].parents);
        assert_eq!(vec![String::from("main")], commits[0].bookmarks);
        assert_eq!("say \"hi\"", commits[0].desc);
        assert_eq!("draft", commits[0].phase);
        assert_eq!(Some(12), commits[0].github_pr);
        assert_eq!(None, commits[1].github_pr);
        assert_eq!(
            vec![
                CommitDigest(String::from("aaa")),
                CommitDigest(String::from("ddd")),
            ],
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
        let graph: String = view
            .rows
            .iter()
            .map(|row| row.graph.as_str())
            .collect::<Vec<&str>>()
            .join("\n");
        // Same fork Sapling draws: the side commit joins with ├─╯.
        assert_eq!(true, graph.contains("├─╯"));
        let commits = parse_smartlog(&bytes).unwrap();
        let mut by_desc: Vec<(String, CommitDigest, Vec<CommitDigest>)> = commits
            .iter()
            .map(|c| (c.desc.clone(), c.node.clone(), c.parents.clone()))
            .collect();
        by_desc.sort();
        assert_eq!(
            vec![
                (
                    String::from("left"),
                    CommitDigest(left),
                    vec![CommitDigest(root.clone())],
                ),
                (
                    String::from("right"),
                    CommitDigest(right),
                    vec![CommitDigest(root.clone())],
                ),
                (String::from("root"), CommitDigest(root), Vec::new()),
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
            node: CommitDigest(String::from(node)),
            parents: Vec::new(),
            phase: String::from("draft"),
            bookmarks: Vec::new(),
            desc: String::from("d"),
            github_pr: None,
            author: String::new(),
            date_unix: TimestampsSecs(0),
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

    fn sl_commit(repo: &TestRepo, message: &str) -> String {
        repo.ok("sl", &["addremove"]);
        repo.ok("sl", &["commit", "-m", message]);
        let hash = repo.ok("sl", &["log", "-r", ".", "-T", "{node}\n"]);
        hash.trim().to_owned()
    }
}
