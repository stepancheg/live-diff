use std::fs;
use std::path::Path;

use anyhow::anyhow;
use anyhow::bail;

/// Diff command for the repository that contains `dir`.
///
/// Sapling is used when the repository already has sl metadata (a `.sl`
/// directory, or `.git/sl`). Probing does not run `sl`: `sl root` creates
/// `.git/sl` in a plain git repository. Otherwise the repository is
/// treated as git.
/// Git and sl default to 3 lines of unified context.
pub(crate) const DEFAULT_CONTEXT: u32 = 3;
/// Upper bound so holding `U` cannot request an enormous diff.
pub(crate) const MAX_CONTEXT: u32 = 999;

#[cfg_attr(not(test), expect(dead_code))]
pub(crate) fn diff_command_in(dir: &Path) -> anyhow::Result<Vec<String>> {
    let tool = diff_tool_in(dir)?;
    Ok(command_for_tool(tool, DEFAULT_CONTEXT, None))
}

pub(crate) fn diff_tool_in(dir: &Path) -> anyhow::Result<DiffTool> {
    repo_kind(dir)
}

pub(crate) fn diff_tool() -> anyhow::Result<DiffTool> {
    let dir = std::env::current_dir()?;
    diff_tool_in(&dir)
}

pub(crate) fn command_for_tool(tool: DiffTool, context: u32, rev: Option<&str>) -> Vec<String> {
    match tool {
        DiffTool::Sl => sl_diff_command(context, rev),
        DiffTool::Git => git_diff_command(context, rev),
    }
}

pub(crate) fn sl_diff_command(context: u32, rev: Option<&str>) -> Vec<String> {
    let mut cmd = vec![
        String::from("sl"),
        String::from("diff"),
        String::from("-U"),
        context.to_string(),
    ];
    if let Some(rev) = rev {
        cmd.push(String::from("-r"));
        cmd.push(rev.to_owned());
    }
    cmd
}

pub(crate) fn git_diff_command(context: u32, rev: Option<&str>) -> Vec<String> {
    let mut cmd = vec![
        String::from("git"),
        String::from("diff"),
        String::from("--no-ext-diff"),
        String::from("-U"),
        context.to_string(),
    ];
    if let Some(rev) = rev {
        cmd.push(rev.to_owned());
    }
    cmd
}

/// Next context size after `u` (decrease) or `U` (increase).
/// Stays put at 0 and at [`MAX_CONTEXT`].
pub(crate) fn step_context(context: u32, increase: bool) -> u32 {
    if increase {
        context.saturating_add(1).min(MAX_CONTEXT)
    } else {
        context.saturating_sub(1)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DiffTool {
    Sl,
    Git,
}

fn repo_kind(start: &Path) -> anyhow::Result<DiffTool> {
    for dir in start.ancestors() {
        if dir.join(".sl").is_dir() {
            return Ok(DiffTool::Sl);
        }
        let dot_git = dir.join(".git");
        if dot_git.is_dir() {
            if dot_git.join("sl").is_dir() {
                return Ok(DiffTool::Sl);
            }
            return Ok(DiffTool::Git);
        }
        if dot_git.is_file() {
            if gitfile_has_sl(&dot_git)? {
                return Ok(DiffTool::Sl);
            }
            return Ok(DiffTool::Git);
        }
    }
    bail!("not an sl or git repository")
}

/// A `.git` file (git worktree or submodule) points at the real git dir.
/// Sapling metadata lives in that dir's `sl` folder, or in the common dir.
fn gitfile_has_sl(gitfile: &Path) -> anyhow::Result<bool> {
    let text = fs::read_to_string(gitfile)?;
    let Some(line) = text.lines().next() else {
        return Ok(false);
    };
    let Some(rest) = line.strip_prefix("gitdir:") else {
        return Ok(false);
    };
    let gitdir_raw = Path::new(rest.trim());
    let gitdir = if gitdir_raw.is_absolute() {
        gitdir_raw.to_path_buf()
    } else {
        let Some(parent) = gitfile.parent() else {
            return Err(anyhow!("gitdir path has no parent"));
        };
        parent.join(gitdir_raw)
    };
    if gitdir.join("sl").is_dir() {
        return Ok(true);
    }
    let commondir_file = gitdir.join("commondir");
    if commondir_file.is_file() {
        let rel = fs::read_to_string(&commondir_file)?;
        let common = gitdir.join(rel.trim());
        if common.join("sl").is_dir() {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::repo::diff_command_in;

    fn sl_argv() -> Vec<String> {
        crate::repo::sl_diff_command(crate::repo::DEFAULT_CONTEXT, None)
    }

    fn git_argv() -> Vec<String> {
        crate::repo::git_diff_command(crate::repo::DEFAULT_CONTEXT, None)
    }

    #[test]
    fn test_step_context_clamps() {
        assert_eq!(0, crate::repo::step_context(0, false));
        assert_eq!(1, crate::repo::step_context(0, true));
        assert_eq!(2, crate::repo::step_context(3, false));
        assert_eq!(4, crate::repo::step_context(3, true));
        assert_eq!(
            crate::repo::MAX_CONTEXT,
            crate::repo::step_context(crate::repo::MAX_CONTEXT, true)
        );
    }

    #[test]
    fn test_command_includes_unified_context() {
        assert_eq!(
            vec![
                String::from("sl"),
                String::from("diff"),
                String::from("-U"),
                String::from("5"),
            ],
            crate::repo::sl_diff_command(5, None)
        );
        assert_eq!(
            vec![
                String::from("git"),
                String::from("diff"),
                String::from("--no-ext-diff"),
                String::from("-U"),
                String::from("8"),
            ],
            crate::repo::git_diff_command(8, None)
        );
        assert_eq!(
            vec![
                String::from("sl"),
                String::from("diff"),
                String::from("-U"),
                String::from("3"),
                String::from("-r"),
                String::from(".^"),
            ],
            crate::repo::sl_diff_command(3, Some(".^"))
        );
        assert_eq!(
            vec![
                String::from("git"),
                String::from("diff"),
                String::from("--no-ext-diff"),
                String::from("-U"),
                String::from("3"),
                String::from("main"),
            ],
            crate::repo::git_diff_command(3, Some("main"))
        );
    }

    #[test]
    fn test_git_sl_dir_selects_sl() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".git/sl")).unwrap();
        let sub = root.join("src/nested");
        fs::create_dir_all(&sub).unwrap();
        assert_eq!(sl_argv(), diff_command_in(&sub).unwrap());
    }

    #[test]
    fn test_dot_sl_dir_selects_sl() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".sl")).unwrap();
        assert_eq!(sl_argv(), diff_command_in(root).unwrap());
    }

    #[test]
    fn test_plain_git_selects_git() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".git")).unwrap();
        assert_eq!(git_argv(), diff_command_in(root).unwrap());
    }

    #[test]
    fn test_sl_preferred_over_git_at_same_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(root.join(".sl")).unwrap();
        assert_eq!(sl_argv(), diff_command_in(root).unwrap());
    }

    #[test]
    fn test_worktree_gitfile_with_sl() {
        let tmp = tempfile::tempdir().unwrap();
        let main_git = tmp.path().join("main/.git");
        fs::create_dir_all(main_git.join("sl")).unwrap();
        fs::create_dir_all(main_git.join("worktrees/wt")).unwrap();
        fs::write(main_git.join("worktrees/wt/commondir"), "../..\n").unwrap();
        let wt = tmp.path().join("wt");
        fs::create_dir_all(&wt).unwrap();
        let gitdir = main_git.join("worktrees/wt");
        fs::write(wt.join(".git"), format!("gitdir: {}\n", gitdir.display())).unwrap();
        assert_eq!(sl_argv(), diff_command_in(&wt).unwrap());
    }

    #[test]
    fn test_worktree_gitfile_without_sl_selects_git() {
        let tmp = tempfile::tempdir().unwrap();
        let main_git = tmp.path().join("main/.git");
        fs::create_dir_all(main_git.join("worktrees/wt")).unwrap();
        fs::write(main_git.join("worktrees/wt/commondir"), "../..\n").unwrap();
        let wt = tmp.path().join("wt");
        fs::create_dir_all(&wt).unwrap();
        let gitdir = main_git.join("worktrees/wt");
        fs::write(wt.join(".git"), format!("gitdir: {}\n", gitdir.display())).unwrap();
        assert_eq!(git_argv(), diff_command_in(&wt).unwrap());
    }

    #[test]
    fn test_no_repo_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let err = diff_command_in(tmp.path()).unwrap_err();
        assert_eq!(
            true,
            err.to_string().contains("not an sl or git repository")
        );
    }

    #[test]
    fn test_stops_at_git_root_without_sl() {
        // A plain git repo must not pick up an unrelated .sl above it.
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join(".sl")).unwrap();
        let inner = tmp.path().join("inner");
        fs::create_dir_all(inner.join(".git")).unwrap();
        let nested = inner.join("src");
        fs::create_dir_all(&nested).unwrap();
        assert_eq!(git_argv(), diff_command_in(&nested).unwrap());
    }
}
