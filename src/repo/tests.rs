use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;

use crate::diff::DiffOutput;
use crate::diff::FileDiff;
use crate::diff::HunkLineKind;
use crate::diff::parse_diff;
use crate::repo::git::git_commit_diff_command;
use crate::repo::git::git_diff_command;
use crate::repo::git::git_untracked_command;
use crate::repo::repo::parse_untracked_paths;
use crate::repo::sl::sl_commit_diff_command;
use crate::repo::sl::sl_diff_command;
use crate::repo::sl::sl_untracked_command;

/// Which repository the shared diff-command scenarios run against.
///
/// `Git` runs the git commands in a plain git repo. `Sl` runs the sl
/// commands in a native `.sl` repo. `SlOnGit` runs the sl commands in a
/// git repo with Sapling metadata in `.git/sl` (`sl init --git` is a
/// different layout: a `.sl` repo whose store is git).
#[derive(Clone, Copy)]
enum Kind {
    Git,
    Sl,
    SlOnGit,
}

#[test]
fn test_git_diff_command_worktree() {
    check_diff_command_worktree(&Repo::init(Kind::Git));
}

#[test]
fn test_sl_diff_command_worktree_sl_mode() {
    check_diff_command_worktree(&Repo::init(Kind::Sl));
}

#[test]
fn test_sl_diff_command_worktree_git_mode() {
    check_diff_command_worktree(&Repo::init(Kind::SlOnGit));
}

#[test]
fn test_git_diff_command_context() {
    check_diff_command_context(&Repo::init(Kind::Git));
}

#[test]
fn test_sl_diff_command_context_sl_mode() {
    check_diff_command_context(&Repo::init(Kind::Sl));
}

#[test]
fn test_sl_diff_command_context_git_mode() {
    check_diff_command_context(&Repo::init(Kind::SlOnGit));
}

#[test]
fn test_git_diff_command_rev() {
    check_diff_command_rev(&Repo::init(Kind::Git));
}

#[test]
fn test_sl_diff_command_rev_sl_mode() {
    check_diff_command_rev(&Repo::init(Kind::Sl));
}

#[test]
fn test_sl_diff_command_rev_git_mode() {
    check_diff_command_rev(&Repo::init(Kind::SlOnGit));
}

#[test]
fn test_git_commit_diff_command_one_commit() {
    check_commit_diff_command_one_commit(&Repo::init(Kind::Git));
}

#[test]
fn test_sl_commit_diff_command_one_commit_sl_mode() {
    check_commit_diff_command_one_commit(&Repo::init(Kind::Sl));
}

#[test]
fn test_sl_commit_diff_command_one_commit_git_mode() {
    check_commit_diff_command_one_commit(&Repo::init(Kind::SlOnGit));
}

#[test]
fn test_git_commit_diff_command_root() {
    check_commit_diff_command_root(&Repo::init(Kind::Git));
}

#[test]
fn test_sl_commit_diff_command_root_sl_mode() {
    check_commit_diff_command_root(&Repo::init(Kind::Sl));
}

#[test]
fn test_sl_commit_diff_command_root_git_mode() {
    check_commit_diff_command_root(&Repo::init(Kind::SlOnGit));
}

#[test]
fn test_git_commit_diff_command_rename() {
    check_commit_diff_command_rename(&Repo::init(Kind::Git));
}

#[test]
fn test_sl_commit_diff_command_rename_sl_mode() {
    check_commit_diff_command_rename(&Repo::init(Kind::Sl));
}

#[test]
fn test_sl_commit_diff_command_rename_git_mode() {
    check_commit_diff_command_rename(&Repo::init(Kind::SlOnGit));
}

#[test]
fn test_git_untracked_command_lists_paths() {
    check_untracked_command_lists_paths(&Repo::init(Kind::Git));
}

#[test]
fn test_sl_untracked_command_lists_paths_sl_mode() {
    check_untracked_command_lists_paths(&Repo::init(Kind::Sl));
}

#[test]
fn test_sl_untracked_command_lists_paths_git_mode() {
    check_untracked_command_lists_paths(&Repo::init(Kind::SlOnGit));
}

fn check_diff_command_worktree(repo: &Repo) {
    let dir = repo.path();
    write_file(dir, "a.txt", "one\ntwo\nthree\n");
    repo.commit("add a");
    let clean = repo.run_diff(repo.diff_command(3, None));
    assert_eq!(0, clean.files().len());

    write_file(dir, "a.txt", "one\nTWO\nthree\n");
    write_file(dir, "untracked.txt", "nope\n");
    let dirty = repo.run_diff(repo.diff_command(3, None));
    let files = dirty.files();
    assert_eq!(1, files.len());
    assert_eq!(Some(String::from("a.txt")), files[0].old);
    assert_eq!(Some(String::from("a.txt")), files[0].new);
    assert_eq!(
        vec![
            (HunkLineKind::Context, String::from("one")),
            (HunkLineKind::Delete, String::from("two")),
            (HunkLineKind::Insert, String::from("TWO")),
            (HunkLineKind::Context, String::from("three")),
        ],
        hunk_lines(files[0])
    );
}

fn check_diff_command_context(repo: &Repo) {
    let dir = repo.path();
    write_file(dir, "a.txt", "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\n");
    repo.commit("add a");
    write_file(dir, "a.txt", "l1\nl2\nl3\nl4\nL5\nl6\nl7\nl8\nl9\n");

    let narrow = repo.run_diff(repo.diff_command(0, None));
    let narrow_files = narrow.files();
    assert_eq!(1, narrow_files.len());
    assert_eq!(
        vec![
            (HunkLineKind::Delete, String::from("l5")),
            (HunkLineKind::Insert, String::from("L5")),
        ],
        hunk_lines(narrow_files[0])
    );

    let wide = repo.run_diff(repo.diff_command(2, None));
    let wide_files = wide.files();
    assert_eq!(1, wide_files.len());
    assert_eq!(
        vec![
            (HunkLineKind::Context, String::from("l3")),
            (HunkLineKind::Context, String::from("l4")),
            (HunkLineKind::Delete, String::from("l5")),
            (HunkLineKind::Insert, String::from("L5")),
            (HunkLineKind::Context, String::from("l6")),
            (HunkLineKind::Context, String::from("l7")),
        ],
        hunk_lines(wide_files[0])
    );
}

fn check_diff_command_rev(repo: &Repo) {
    let dir = repo.path();
    write_file(dir, "a.txt", "base\n");
    let c1 = repo.commit("base");
    write_file(dir, "a.txt", "base\nsecond\n");
    repo.commit("second");
    write_file(dir, "a.txt", "base\nsecond\nworktree\n");

    let since_c1 = repo.run_diff(repo.diff_command(3, Some(&c1)));
    let files = since_c1.files();
    assert_eq!(1, files.len());
    assert_eq!(
        vec![
            (HunkLineKind::Context, String::from("base")),
            (HunkLineKind::Insert, String::from("second")),
            (HunkLineKind::Insert, String::from("worktree")),
        ],
        hunk_lines(files[0])
    );

    let worktree_only = repo.run_diff(repo.diff_command(3, None));
    let worktree_files = worktree_only.files();
    assert_eq!(1, worktree_files.len());
    assert_eq!(
        vec![
            (HunkLineKind::Context, String::from("base")),
            (HunkLineKind::Context, String::from("second")),
            (HunkLineKind::Insert, String::from("worktree")),
        ],
        hunk_lines(worktree_files[0])
    );
}

fn check_commit_diff_command_one_commit(repo: &Repo) {
    let dir = repo.path();
    write_file(dir, "a.txt", "one\ntwo\nthree\n");
    repo.commit("first");
    write_file(dir, "a.txt", "one\nTWO\nthree\n");
    let c2 = repo.commit("second");
    write_file(dir, "a.txt", "one\nTWO\nTHREE\n");
    repo.commit("third");
    write_file(dir, "a.txt", "one\nTWO\nTHREE\nworktree\n");

    let diff = repo.run_diff(repo.commit_diff_command(3, &c2));
    let files = diff.files();
    assert_eq!(1, files.len());
    assert_eq!(Some(String::from("a.txt")), files[0].old);
    assert_eq!(Some(String::from("a.txt")), files[0].new);
    assert_eq!(
        vec![
            (HunkLineKind::Context, String::from("one")),
            (HunkLineKind::Delete, String::from("two")),
            (HunkLineKind::Insert, String::from("TWO")),
            (HunkLineKind::Context, String::from("three")),
        ],
        hunk_lines(files[0])
    );
}

fn check_commit_diff_command_root(repo: &Repo) {
    let dir = repo.path();
    write_file(dir, "a.txt", "one\ntwo\n");
    let c1 = repo.commit("root");
    write_file(dir, "a.txt", "one\ntwo\nworktree\n");

    let diff = repo.run_diff(repo.commit_diff_command(3, &c1));
    let files = diff.files();
    assert_eq!(1, files.len());
    assert_eq!(None, files[0].old);
    assert_eq!(Some(String::from("a.txt")), files[0].new);
    assert_eq!(
        vec![
            (HunkLineKind::Insert, String::from("one")),
            (HunkLineKind::Insert, String::from("two")),
        ],
        hunk_lines(files[0])
    );
}

fn check_commit_diff_command_rename(repo: &Repo) {
    let dir = repo.path();
    write_file(dir, "old.txt", "hello\nworld\n");
    repo.commit("add");
    fs::rename(dir.join("old.txt"), dir.join("new.txt")).unwrap();
    let c2 = repo.commit("rename");

    let diff = repo.run_diff(repo.commit_diff_command(3, &c2));
    let files = diff.files();
    assert_eq!(1, files.len());
    assert_eq!(Some(String::from("old.txt")), files[0].old);
    assert_eq!(Some(String::from("new.txt")), files[0].new);
    assert_eq!(0, files[0].hunks.len());
}

fn check_untracked_command_lists_paths(repo: &Repo) {
    let dir = repo.path();
    write_file(dir, "tracked.txt", "t\n");
    write_file(dir, ".gitignore", "ignored.txt\n");
    repo.commit("tracked");
    write_file(dir, "untracked.txt", "u\n");
    write_file(dir, "sub/nested.txt", "n\n");
    write_file(dir, "ignored.txt", "i\n");

    let bytes = repo.run_ok(&repo.untracked_command());
    assert_eq!(
        vec![
            String::from("sub/nested.txt"),
            String::from("untracked.txt")
        ],
        parse_untracked_paths(&bytes)
    );
}

/// Working tree plus an isolated HOME so git and sl do not read the
/// real user config.
struct Repo {
    _dir: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    kind: Kind,
}

impl Repo {
    fn init(kind: Kind) -> Repo {
        let repo = Repo::prepare(kind);
        match kind {
            Kind::Git => {
                repo.ok("git", &["-c", "init.defaultBranch=master", "init", "-q"]);
                assert_eq!(true, repo.path().join(".git").is_dir());
                assert_eq!(false, repo.path().join(".sl").exists());
            }
            Kind::Sl => {
                repo.ok("sl", &["init", "-q"]);
                assert_eq!(true, repo.path().join(".sl").is_dir());
                assert_eq!(false, repo.path().join(".git").exists());
            }
            Kind::SlOnGit => {
                repo.ok("git", &["-c", "init.defaultBranch=master", "init", "-q"]);
                // Any sl command in a git repo creates `.git/sl`.
                repo.ok("sl", &["status"]);
                assert_eq!(true, repo.path().join(".git/sl").is_dir());
                assert_eq!(false, repo.path().join(".sl").exists());
            }
        }
        repo
    }

    fn prepare(kind: Kind) -> Repo {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let root = dir.path().join("repo");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&root).unwrap();
        let repo = Repo {
            _dir: dir,
            root,
            home,
            kind,
        };
        match kind {
            Kind::Git => {}
            Kind::Sl => write_user_sl_config(&repo),
            Kind::SlOnGit => write_user_sl_config(&repo),
        }
        repo
    }

    fn path(&self) -> &Path {
        &self.root
    }

    fn home(&self) -> &Path {
        &self.home
    }

    fn diff_command(&self, context: u32, rev: Option<&str>) -> Vec<String> {
        match self.kind {
            Kind::Git => git_diff_command(context, rev),
            Kind::Sl => sl_diff_command(context, rev),
            Kind::SlOnGit => sl_diff_command(context, rev),
        }
    }

    fn commit_diff_command(&self, context: u32, commit: &str) -> Vec<String> {
        match self.kind {
            Kind::Git => git_commit_diff_command(context, commit),
            Kind::Sl => sl_commit_diff_command(context, commit),
            Kind::SlOnGit => sl_commit_diff_command(context, commit),
        }
    }

    fn untracked_command(&self) -> Vec<String> {
        match self.kind {
            Kind::Git => git_untracked_command(),
            Kind::Sl => sl_untracked_command(),
            Kind::SlOnGit => sl_untracked_command(),
        }
    }

    fn commit(&self, message: &str) -> String {
        match self.kind {
            Kind::Git => {
                self.ok("git", &["add", "-A"]);
                self.ok("git", &["commit", "-q", "-m", message]);
                let hash = self.ok("git", &["rev-parse", "HEAD"]);
                hash.trim().to_owned()
            }
            Kind::Sl => self.sl_commit(message),
            Kind::SlOnGit => self.sl_commit(message),
        }
    }

    fn sl_commit(&self, message: &str) -> String {
        // addremove picks up new files and renames; tracked edits are
        // committed from the work tree without an extra add.
        self.ok("sl", &["addremove"]);
        self.ok("sl", &["commit", "-m", message]);
        let hash = self.ok("sl", &["log", "-r", ".", "-T", "{node}\n"]);
        hash.trim().to_owned()
    }

    fn ok(&self, program: &str, args: &[&str]) -> String {
        let stdout = self.run_program(program, args);
        String::from_utf8(stdout).unwrap()
    }

    fn run_diff(&self, argv: Vec<String>) -> DiffOutput {
        let stdout = self.run_ok(&argv);
        parse_diff(&stdout)
    }

    fn run_ok(&self, argv: &[String]) -> Vec<u8> {
        let Some((program, args)) = argv.split_first() else {
            panic!("empty command");
        };
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        self.run_program(program, &arg_refs)
    }

    fn run_program(&self, program: &str, args: &[&str]) -> Vec<u8> {
        let mut cmd = Command::new(program);
        cmd.args(args);
        apply_env(&mut cmd, self.path(), self.home());
        let out = cmd.output().unwrap();
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let stdout = String::from_utf8_lossy(&out.stdout);
            panic!("command {program} {args:?} failed\nstdout: {stdout}\nstderr: {stderr}");
        }
        out.stdout
    }
}

fn write_user_sl_config(repo: &Repo) {
    // Ask sl for the per-user config path under this HOME. Builtin
    // config turns fsmonitor on; the user file has to override that
    // for the exact `sl diff` argv under test, which cannot pass
    // `--config`.
    // cwd is HOME, not the repo dir, so configfile cannot latch onto
    // a parent repository.
    let mut cmd = Command::new("sl");
    cmd.args(["configfile"]);
    apply_env(&mut cmd, repo.home(), repo.home());
    let out = cmd.output().unwrap();
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        panic!("sl configfile failed: {stderr}");
    }
    let text = String::from_utf8(out.stdout).unwrap();
    let mut user_path = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("User config path:") {
            user_path = Some(PathBuf::from(rest.trim()));
        }
    }
    let Some(user_path) = user_path else {
        panic!("sl configfile did not print a user config path:\n{text}");
    };
    if let Some(parent) = user_path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(
        &user_path,
        "\
[ui]
username = Live Diff Test <live-diff-test@example.com>
color = never
paginate = never
[fsmonitor]
mode = off
",
    )
    .unwrap();
}

fn write_file(dir: &Path, rel: &str, contents: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

fn apply_env(cmd: &mut Command, root: &Path, home: &Path) {
    cmd.current_dir(root).stdin(Stdio::null()).env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        cmd.env("PATH", path);
    }
    cmd.env("HOME", home)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Live Diff Test")
        .env("GIT_AUTHOR_EMAIL", "live-diff-test@example.com")
        .env("GIT_COMMITTER_NAME", "Live Diff Test")
        .env("GIT_COMMITTER_EMAIL", "live-diff-test@example.com")
        .env("GIT_AUTHOR_DATE", "2020-01-01T00:00:00+0000")
        .env("GIT_COMMITTER_DATE", "2020-01-01T00:00:00+0000")
        .env("LC_ALL", "C");
}

fn hunk_lines(file: &FileDiff) -> Vec<(HunkLineKind, String)> {
    let mut lines = Vec::new();
    for hunk in &file.hunks {
        for line in &hunk.lines {
            lines.push((line.kind, line.text.clone()));
        }
    }
    lines
}
