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

/// Commit message and diff introduced by one commit (`sl show`).
pub(crate) fn sl_commit_diff_command(context: u32, commit: &str) -> Vec<String> {
    vec![
        String::from("sl"),
        String::from("show"),
        String::from("-U"),
        context.to_string(),
        String::from("-r"),
        commit.to_owned(),
    ]
}

/// List untracked, non-ignored paths. Output is NUL-separated and
/// relative to the repository root.
pub(crate) fn sl_untracked_command() -> Vec<String> {
    vec![
        String::from("sl"),
        String::from("status"),
        String::from("-u"),
        String::from("-n"),
        String::from("-0"),
        String::from("--root-relative"),
    ]
}
