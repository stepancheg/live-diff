use std::collections::HashMap;
use std::process::Command;
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;

use crate::repo::forge::Forge;

/// Review state painted next to `#N`, matching `sl ssl`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PrReview {
    /// PR is closed. `sl ssl` styles this as committed.
    Closed,
    /// PR is merged.
    Merged,
    Approved,
    ChangesRequested,
    ReviewRequired,
    /// Open, with no review decision.
    Unreviewed,
}

/// CI rollup glyph, matching `sl ssl`'s GitHub signal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PrCheck {
    Ok,
    Pending,
    Failed,
}

/// Status for one pull request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PrStatus {
    pub(crate) review: PrReview,
    pub(crate) check: Option<PrCheck>,
}

impl PrStatus {
    /// Word `sl ssl` prints after the PR link.
    pub(crate) fn review_text(self) -> &'static str {
        match self.review {
            PrReview::Closed => "Closed",
            PrReview::Merged => "Merged",
            PrReview::Approved => "Approved",
            PrReview::ChangesRequested => "Changes Requested",
            PrReview::ReviewRequired => "Review Required",
            PrReview::Unreviewed => "Unreviewed",
        }
    }

    /// Glyph `sl ssl` prints for the check rollup. `None` when there
    /// is no rollup (or it is a state ssl does not draw).
    pub(crate) fn check_glyph(self) -> Option<&'static str> {
        match self.check {
            Some(PrCheck::Ok) => Some("\u{2713}"),
            Some(PrCheck::Pending) => Some("\u{22ef}"),
            Some(PrCheck::Failed) => Some("\u{2717}"),
            None => None,
        }
    }
}

/// How long a fetched status is reused before the tree asks again.
pub(crate) const PR_STATUS_REFRESH: Duration = Duration::from_secs(20);

/// Fetch review and CI status for `numbers`. Missing or failed
/// lookups are omitted so the tree can keep a bare `#N`.
pub(crate) async fn fetch_pr_statuses(forge: &Forge, numbers: &[u64]) -> HashMap<u64, PrStatus> {
    if numbers.is_empty() {
        return HashMap::new();
    }
    match forge {
        Forge::GitHub { owner, repo } => fetch_github_statuses(owner, repo, numbers).await,
        Forge::Origin { org, name } => fetch_origin_statuses(org, name, numbers).await,
    }
}

/// Map a forge PR state plus review decision and check rollup onto
/// the `sl ssl` labels. `state` is `OPEN` / `CLOSED` / `MERGED`
/// (GitHub) or `open` / `closed` / `merged` / `draft` (Origin).
pub(crate) fn pr_status_from_parts(
    state: &str,
    review_decision: &str,
    check: Option<PrCheck>,
) -> PrStatus {
    let state = state.to_ascii_uppercase();
    let review = if state == "CLOSED" {
        PrReview::Closed
    } else if state == "MERGED" {
        PrReview::Merged
    } else {
        // OPEN, DRAFT, or anything else: ssl uses the review decision.
        match review_decision {
            "APPROVED" => PrReview::Approved,
            "CHANGES_REQUESTED" => PrReview::ChangesRequested,
            "REVIEW_REQUIRED" => PrReview::ReviewRequired,
            _ => PrReview::Unreviewed,
        }
    };
    PrStatus { review, check }
}

/// GitHub `statusCheckRollup.state`. ssl draws SUCCESS / PENDING /
/// FAILURE; ERROR is a failed rollup.
pub(crate) fn pr_check_from_github_rollup(state: &str) -> Option<PrCheck> {
    match state {
        "SUCCESS" => Some(PrCheck::Ok),
        "PENDING" | "EXPECTED" => Some(PrCheck::Pending),
        "FAILURE" | "ERROR" => Some(PrCheck::Failed),
        _ => None,
    }
}

async fn fetch_github_statuses(owner: &str, repo: &str, numbers: &[u64]) -> HashMap<u64, PrStatus> {
    let mut out = HashMap::new();
    // One GraphQL request per chunk so a long stack stays under the
    // query size GitHub accepts.
    for chunk in numbers.chunks(20) {
        let query = github_status_graphql_query(chunk);
        let argv = github_status_argv(owner, repo, &query);
        let Some(bytes) = run_command(argv).await else {
            continue;
        };
        for (number, status) in parse_github_status_response(&bytes) {
            out.insert(number, status);
        }
    }
    out
}

fn github_status_graphql_query(numbers: &[u64]) -> String {
    let mut q =
        String::from("query($owner:String!,$name:String!){repository(owner:$owner,name:$name){");
    for number in numbers {
        q.push_str(&format!(
            "pr{number}:pullRequest(number:{number}){{number state reviewDecision commits(last:1){{nodes{{commit{{statusCheckRollup{{state}}}}}}}}}}"
        ));
    }
    q.push_str("}}");
    q
}

fn github_status_argv(owner: &str, repo: &str, query: &str) -> Vec<String> {
    let mut argv = Vec::new();
    argv.push(String::from("gh"));
    argv.push(String::from("api"));
    argv.push(String::from("graphql"));
    argv.push(String::from("-f"));
    argv.push(format!("query={query}"));
    argv.push(String::from("-f"));
    argv.push(format!("owner={owner}"));
    argv.push(String::from("-f"));
    argv.push(format!("name={repo}"));
    argv
}

fn parse_github_status_response(bytes: &[u8]) -> Vec<(u64, PrStatus)> {
    let Ok(root) = serde_json::from_slice::<GithubGraphql>(bytes) else {
        return Vec::new();
    };
    let Some(data) = root.data else {
        return Vec::new();
    };
    let Some(repo) = data.repository else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for pr in repo.0.values() {
        let Some(pr) = pr else {
            continue;
        };
        let check = pr
            .commits
            .as_ref()
            .and_then(|c| c.nodes.first())
            .and_then(|n| n.commit.as_ref())
            .and_then(|c| c.status_check_rollup.as_ref())
            .and_then(|r| r.state.as_deref())
            .and_then(pr_check_from_github_rollup);
        let review = pr.review_decision.as_deref().unwrap_or("");
        let state = pr.state.as_deref().unwrap_or("");
        out.push((pr.number, pr_status_from_parts(state, review, check)));
    }
    out
}

#[derive(Deserialize)]
struct GithubGraphql {
    data: Option<GithubData>,
}

#[derive(Deserialize)]
struct GithubData {
    repository: Option<GithubRepo>,
}

/// Alias map (`pr12`, `pr34`, ...). Null when that PR does not exist.
#[derive(Deserialize)]
struct GithubRepo(HashMap<String, Option<GithubPr>>);

#[derive(Deserialize)]
struct GithubPr {
    number: u64,
    state: Option<String>,
    #[serde(default, rename = "reviewDecision")]
    review_decision: Option<String>,
    commits: Option<GithubCommits>,
}

#[derive(Deserialize)]
struct GithubCommits {
    nodes: Vec<GithubCommitNode>,
}

#[derive(Deserialize)]
struct GithubCommitNode {
    commit: Option<GithubCommit>,
}

#[derive(Deserialize)]
struct GithubCommit {
    #[serde(default, rename = "statusCheckRollup")]
    status_check_rollup: Option<GithubRollup>,
}

#[derive(Deserialize)]
struct GithubRollup {
    state: Option<String>,
}

async fn fetch_origin_statuses(org: &str, name: &str, numbers: &[u64]) -> HashMap<u64, PrStatus> {
    let mut out = HashMap::new();
    for chunk in numbers.chunks(4) {
        let mut handles = Vec::new();
        for number in chunk {
            let org = org.to_owned();
            let name = name.to_owned();
            let number = *number;
            handles.push(tokio::spawn(async move {
                let argv = origin_status_argv(&org, &name, number);
                let bytes = run_command(argv).await?;
                let status = parse_origin_status_response(&bytes)?;
                Some((number, status))
            }));
        }
        for handle in handles {
            match handle.await {
                Ok(Some((number, status))) => {
                    out.insert(number, status);
                }
                Ok(None) => {}
                Err(_) => {}
            }
        }
    }
    out
}

fn origin_status_argv(org: &str, name: &str, number: u64) -> Vec<String> {
    let mut argv = Vec::new();
    argv.push(String::from("origin"));
    argv.push(String::from("pr"));
    argv.push(String::from("view"));
    argv.push(number.to_string());
    argv.push(String::from("-R"));
    argv.push(format!("{org}/{name}"));
    argv.push(String::from("--json"));
    argv.push(String::from("number,status,reviewDecision,ciState"));
    argv
}

fn parse_origin_status_response(bytes: &[u8]) -> Option<PrStatus> {
    let view: OriginView = serde_json::from_slice(bytes).ok()?;
    let check = view.ci_state.as_ref().and_then(origin_check_rollup);
    Some(pr_status_from_parts(
        &view.status,
        &view.review_decision,
        check,
    ))
}

#[derive(Deserialize)]
struct OriginView {
    status: String,
    #[serde(default, rename = "reviewDecision")]
    review_decision: String,
    #[serde(default, rename = "ciState")]
    ci_state: Option<OriginCiState>,
}

#[derive(Deserialize)]
struct OriginCiState {
    #[serde(default, rename = "checkRunGroups")]
    check_run_groups: Vec<OriginCheckGroup>,
}

#[derive(Deserialize)]
struct OriginCheckGroup {
    #[serde(default, rename = "checkRuns")]
    check_runs: Vec<OriginCheckRun>,
}

#[derive(Deserialize)]
struct OriginCheckRun {
    #[serde(default)]
    status: String,
    #[serde(default)]
    conclusion: Option<String>,
}

/// Roll check runs up the way Origin's commit rollup does: a failure
/// wins over pending, pending wins over success. No runs means no
/// glyph.
fn origin_check_rollup(ci: &OriginCiState) -> Option<PrCheck> {
    let mut saw_run = false;
    let mut pending = false;
    let mut failed = false;
    for group in &ci.check_run_groups {
        for run in &group.check_runs {
            saw_run = true;
            let status = run.status.to_ascii_lowercase();
            if status != "completed" {
                // queued, in_progress, rerequested, failing: not done.
                pending = true;
                continue;
            }
            let conclusion = run.conclusion.as_deref().unwrap_or("").to_ascii_lowercase();
            match conclusion.as_str() {
                "success" | "neutral" | "skipped" => {}
                _ => {
                    // failure, cancelled, timed_out, action_required,
                    // stale, or a completed run with no conclusion.
                    failed = true;
                }
            }
        }
    }
    if !saw_run {
        return None;
    }
    if failed {
        return Some(PrCheck::Failed);
    }
    if pending {
        return Some(PrCheck::Pending);
    }
    Some(PrCheck::Ok)
}

async fn run_command(argv: Vec<String>) -> Option<Vec<u8>> {
    let out = tokio::task::spawn_blocking(move || {
        let (program, args) = argv.split_first()?;
        let output = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .env_remove("FORCE_COLOR")
            .env("NO_COLOR", "1")
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        Some(output.stdout)
    })
    .await
    .ok()?;
    out
}

#[cfg(test)]
mod tests {
    use crate::repo::pr_status::PrCheck;
    use crate::repo::pr_status::PrReview;
    use crate::repo::pr_status::PrStatus;
    use crate::repo::pr_status::github_status_argv;
    use crate::repo::pr_status::github_status_graphql_query;
    use crate::repo::pr_status::origin_status_argv;
    use crate::repo::pr_status::parse_github_status_response;
    use crate::repo::pr_status::parse_origin_status_response;
    use crate::repo::pr_status::pr_check_from_github_rollup;
    use crate::repo::pr_status::pr_status_from_parts;

    #[test]
    fn test_pr_status_from_parts_matches_ssl() {
        assert_eq!(
            PrStatus {
                review: PrReview::Merged,
                check: Some(PrCheck::Ok),
            },
            pr_status_from_parts("MERGED", "APPROVED", Some(PrCheck::Ok))
        );
        assert_eq!(
            PrStatus {
                review: PrReview::Closed,
                check: None,
            },
            pr_status_from_parts("closed", "CHANGES_REQUESTED", None)
        );
        assert_eq!(
            PrStatus {
                review: PrReview::Approved,
                check: Some(PrCheck::Pending),
            },
            pr_status_from_parts("OPEN", "APPROVED", Some(PrCheck::Pending))
        );
        assert_eq!(
            PrStatus {
                review: PrReview::ChangesRequested,
                check: Some(PrCheck::Failed),
            },
            pr_status_from_parts("open", "CHANGES_REQUESTED", Some(PrCheck::Failed))
        );
        assert_eq!(
            PrStatus {
                review: PrReview::ReviewRequired,
                check: None,
            },
            pr_status_from_parts("draft", "REVIEW_REQUIRED", None)
        );
        assert_eq!(
            PrStatus {
                review: PrReview::Unreviewed,
                check: None,
            },
            pr_status_from_parts("OPEN", "", None)
        );
        assert_eq!(
            "Approved",
            pr_status_from_parts("OPEN", "APPROVED", None).review_text()
        );
        assert_eq!(
            Some("\u{2713}"),
            PrStatus {
                review: PrReview::Approved,
                check: Some(PrCheck::Ok),
            }
            .check_glyph()
        );
    }

    #[test]
    fn test_pr_check_from_github_rollup() {
        assert_eq!(Some(PrCheck::Ok), pr_check_from_github_rollup("SUCCESS"));
        assert_eq!(
            Some(PrCheck::Pending),
            pr_check_from_github_rollup("PENDING")
        );
        assert_eq!(
            Some(PrCheck::Failed),
            pr_check_from_github_rollup("FAILURE")
        );
        assert_eq!(Some(PrCheck::Failed), pr_check_from_github_rollup("ERROR"));
        assert_eq!(None, pr_check_from_github_rollup(""));
    }

    #[test]
    fn test_parse_github_status_response() {
        let bytes = br#"{
          "data": {
            "repository": {
              "pr12": {
                "number": 12,
                "state": "OPEN",
                "reviewDecision": "APPROVED",
                "commits": {
                  "nodes": [
                    {"commit": {"statusCheckRollup": {"state": "SUCCESS"}}}
                  ]
                }
              },
              "pr13": null
            }
          }
        }"#;
        let got = parse_github_status_response(bytes);
        assert_eq!(1, got.len());
        assert_eq!(12, got[0].0);
        assert_eq!(
            PrStatus {
                review: PrReview::Approved,
                check: Some(PrCheck::Ok),
            },
            got[0].1
        );
    }

    #[test]
    fn test_parse_origin_status_response() {
        let bytes = br#"{
          "number": 7,
          "status": "open",
          "reviewDecision": "CHANGES_REQUESTED",
          "ciState": {
            "checkRunGroups": [
              {
                "checkRuns": [
                  {"status": "completed", "conclusion": "success"},
                  {"status": "completed", "conclusion": "failure"}
                ]
              }
            ]
          }
        }"#;
        assert_eq!(
            Some(PrStatus {
                review: PrReview::ChangesRequested,
                check: Some(PrCheck::Failed),
            }),
            parse_origin_status_response(bytes)
        );
        let pending = br#"{
          "status": "open",
          "reviewDecision": "",
          "ciState": {
            "checkRunGroups": [
              {"checkRuns": [{"status": "in_progress", "conclusion": null}]}
            ]
          }
        }"#;
        assert_eq!(
            Some(PrStatus {
                review: PrReview::Unreviewed,
                check: Some(PrCheck::Pending),
            }),
            parse_origin_status_response(pending)
        );
        let empty =
            br#"{"status":"merged","reviewDecision":"APPROVED","ciState":{"checkRunGroups":[]}}"#;
        assert_eq!(
            Some(PrStatus {
                review: PrReview::Merged,
                check: None,
            }),
            parse_origin_status_response(empty)
        );
    }

    #[test]
    fn test_status_command_argv() {
        assert_eq!(
            vec![
                String::from("gh"),
                String::from("api"),
                String::from("graphql"),
                String::from("-f"),
                String::from("query=Q"),
                String::from("-f"),
                String::from("owner=stepancheg"),
                String::from("-f"),
                String::from("name=sl-tui"),
            ],
            github_status_argv("stepancheg", "sl-tui", "Q")
        );
        let query = github_status_graphql_query(&[12, 34]);
        assert_eq!(true, query.contains("pr12:pullRequest(number:12)"));
        assert_eq!(true, query.contains("pr34:pullRequest(number:34)"));
        assert_eq!(
            vec![
                String::from("origin"),
                String::from("pr"),
                String::from("view"),
                String::from("9"),
                String::from("-R"),
                String::from("spacexsi/xai"),
                String::from("--json"),
                String::from("number,status,reviewDecision,ciState"),
            ],
            origin_status_argv("spacexsi", "xai", 9)
        );
    }
}
