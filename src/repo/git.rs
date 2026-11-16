/// Diff introduced by one commit, the git form of `sl diff -c REV`.
///
/// `git show` prints that patch, including a root commit against the
/// empty tree. `git diff REV^!` does the same when `REV` has a parent,
/// but a root commit's `REV^!` is just `REV`, and `git diff REV` then
/// compares that tree to the work tree.
pub(crate) fn git_commit_diff_command(context: u32, commit: &str) -> Vec<String> {
    vec![
        String::from("git"),
        String::from("show"),
        String::from("--format="),
        String::from("--no-ext-diff"),
        format!("--unified={context}"),
        commit.to_owned(),
    ]
}

pub(crate) fn git_diff_command(context: u32, rev: Option<&str>) -> Vec<String> {
    // One argv element. `git diff -U N` is two arguments; git 2.55 treats
    // `-U` like `-u` (a synonym for `-p`) and then reads `N` as a revision.
    // `--unified=N` is accepted on both old and new git.
    let mut cmd = vec![
        String::from("git"),
        String::from("diff"),
        String::from("--no-ext-diff"),
        format!("--unified={context}"),
    ];
    if let Some(rev) = rev {
        cmd.push(rev.to_owned());
    }
    cmd
}

/// List untracked, non-ignored paths. Output is NUL-separated and
/// relative to the repository root.
pub(crate) fn git_untracked_command() -> Vec<String> {
    vec![
        String::from("git"),
        String::from("ls-files"),
        String::from("--others"),
        String::from("--exclude-standard"),
        String::from("--full-name"),
        String::from("-z"),
    ]
}
