/// `origin pr diff` for pull request `number`.
///
/// `--color never`: the pager re-styles the diff, and `gh`/`origin`
/// colorize when `FORCE_COLOR` is set even though stdout is a pipe.
pub(crate) fn origin_pr_diff_command(number: u64) -> Vec<String> {
    pr_diff_command("origin", number)
}

/// `gh pr diff` for pull request `number`.
pub(crate) fn github_pr_diff_command(number: u64) -> Vec<String> {
    pr_diff_command("gh", number)
}

fn pr_diff_command(program: &str, number: u64) -> Vec<String> {
    let mut cmd = Vec::new();
    cmd.push(program.to_owned());
    cmd.push(String::from("pr"));
    cmd.push(String::from("diff"));
    cmd.push(String::from("--color"));
    cmd.push(String::from("never"));
    cmd.push(number.to_string());
    cmd
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    use crate::diff::HunkLineKind;
    use crate::diff::parse_diff;
    use crate::pr_diff::github_pr_diff_command;
    use crate::pr_diff::origin_pr_diff_command;

    #[test]
    fn test_origin_pr_diff_command_runs() {
        check_pr_diff_command("origin", origin_pr_diff_command(41));
    }

    #[test]
    fn test_github_pr_diff_command_runs() {
        check_pr_diff_command("gh", github_pr_diff_command(41));
    }

    fn check_pr_diff_command(program: &str, argv: Vec<String>) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(program);
        // The fake accepts only the argv we build and prints a minimal
        // git diff. A wrong flag or a missing PR number fails the run.
        let script = format!(
            "#!/bin/sh\n\
             if [ \"$*\" != \"pr diff --color never 41\" ]; then\n\
             echo \"unexpected args: $*\" >&2\n\
             exit 2\n\
             fi\n\
             printf '%s\\n' \\\n\
             'diff --git a/foo b/foo' \\\n\
             '--- a/foo' \\\n\
             '+++ b/foo' \\\n\
             '@@ -1 +1 @@' \\\n\
             '-old' \\\n\
             '+new'\n"
        );
        fs::write(&path, script).unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();

        let path_env = match std::env::var_os("PATH") {
            Some(path_env) => {
                let mut paths = OsString::from(dir.path().as_os_str());
                paths.push(":");
                paths.push(path_env);
                paths
            }
            None => dir.path().as_os_str().to_owned(),
        };
        let out = Command::new(&argv[0])
            .args(&argv[1..])
            .env("PATH", path_env)
            .output()
            .unwrap();
        assert_eq!(true, out.status.success());
        let parsed = parse_diff(&out.stdout);
        let files = parsed.files();
        assert_eq!(1, files.len());
        assert_eq!(Some(String::from("foo")), files[0].new);
        assert_eq!(
            vec![
                (HunkLineKind::Delete, String::from("old")),
                (HunkLineKind::Insert, String::from("new")),
            ],
            files[0].hunks[0]
                .lines
                .iter()
                .map(|line| (line.kind, line.text.clone()))
                .collect::<Vec<_>>()
        );
    }
}
