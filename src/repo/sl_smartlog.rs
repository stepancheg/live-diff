use std::str;

/// Null parent hash emitted by `p1node` / `p2node` when that parent
/// does not exist.
const NULL_NODE: &str = "0000000000000000000000000000000000000000";

/// One commit from [`sl_smartlog_command`], enough to rebuild the
/// parent DAG `sl sl` draws.
#[cfg_attr(not(test), expect(dead_code))]
pub(crate) struct SmartlogCommit {
    pub(crate) node: String,
    /// Present parents only, first parent then second.
    pub(crate) parents: Vec<String>,
    pub(crate) phase: String,
    pub(crate) bookmarks: Vec<String>,
    pub(crate) desc: String,
    /// GitHub pull request number when the commit is tied to one.
    pub(crate) github_pr: Option<u64>,
}

/// `sl log -r smartlog()` as one JSON object per line.
#[cfg_attr(not(test), expect(dead_code))]
pub(crate) fn sl_smartlog_command() -> Vec<String> {
    // The trailing newline keeps one JSON object per line.
    let mut cmd = Vec::new();
    cmd.push(String::from("sl"));
    cmd.push(String::from("log"));
    cmd.push(String::from("-r"));
    cmd.push(String::from("smartlog()"));
    cmd.push(String::from("-T"));
    cmd.push(String::from(
        "{dict(node=node, p1=p1node, p2=p2node, phase=phase, bookmarks=bookmarks, desc=desc|firstline, pr=github_pull_request_number)|json}\n",
    ));
    cmd
}

/// Parse NDJSON from [`sl_smartlog_command`].
#[cfg_attr(not(test), expect(dead_code))]
pub(crate) fn parse_smartlog(bytes: &[u8]) -> anyhow::Result<Vec<SmartlogCommit>> {
    let text = str::from_utf8(bytes)?;
    let mut commits = Vec::new();
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        commits.push(parse_smartlog_line(line)?);
    }
    Ok(commits)
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
    })
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

    fn sl_commit(repo: &TestRepo, message: &str) -> String {
        repo.ok("sl", &["addremove"]);
        repo.ok("sl", &["commit", "-m", message]);
        let hash = repo.ok("sl", &["log", "-r", ".", "-T", "{node}\n"]);
        hash.trim().to_owned()
    }
}
