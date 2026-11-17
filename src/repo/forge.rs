use std::process::Command;

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

/// Pick Origin when a remote is `origin.cursor.com`, otherwise GitHub.
pub(crate) fn detect_forge() -> Option<Forge> {
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
fn forge_from_remote_url(url: &str) -> Option<Forge> {
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

#[cfg(test)]
mod tests {
    use crate::repo::forge::Forge;
    use crate::repo::forge::forge_from_remote_url;

    #[test]
    fn test_forge_from_remote_url() {
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
}
