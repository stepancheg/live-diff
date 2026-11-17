mod abort_on_drop_handle;
mod capture;
mod debug_or_test;
mod diff;
mod event_loop;
mod help;
mod interval_arg;
mod lines_with_wrapped_row_count;
mod mark;
mod pr_diff;
mod render;
mod repo;
mod search;
mod smartlog;
mod terminal;
mod terminal_bg;
mod terminal_guard;
mod theme;
mod util;
mod wrapped_row;

use std::io;
use std::panic;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use std::time::Duration;

use clap::Parser;
use crossterm::event::Event;
use tokio::sync::mpsc;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::abort_on_drop_handle::AbortOnDropHandle;
use crate::capture::CaptureSpec;
use crate::capture::capture_command_output_loop;
use crate::event_loop::EventLoop;
use crate::event_loop::StartView;
use crate::interval_arg::IntervalArg;
use crate::pr_diff::github_pr_diff_command;
use crate::pr_diff::origin_pr_diff_command;
use crate::render::terminal_size;
use crate::repo::repo::DEFAULT_CONTEXT;
use crate::repo::repo::DiffTool;
use crate::repo::repo::MAX_CONTEXT;
use crate::repo::repo::diff_tool;
use crate::search::history::store::History;
use crate::terminal_bg::query_terminal_bg;
use crate::terminal_guard::TerminalGuard;
use crate::terminal_guard::restore_terminal;
use crate::util::notify_pair::NotifyTx;

#[derive(clap::Parser, Debug)]
struct Args {
    /// Seconds between command runs.
    #[clap(short = 'n', long, env = "LIVE_DIFF_INTERVAL", default_value = "0.5")]
    interval: IntervalArg,

    /// Use `sl diff`.
    #[clap(long, conflicts_with = "git")]
    sl: bool,

    /// Use `git diff`.
    #[clap(long, conflicts_with = "sl")]
    git: bool,

    /// Revision to compare the working copy against.
    /// Without this flag, run the tool's default diff.
    #[clap(
        short = 'r',
        long = "rev",
        conflicts_with = "commit",
        conflicts_with = "origin_pr",
        conflicts_with = "github_pr"
    )]
    rev: Option<String>,

    /// Show this commit's message and diff (`sl show`, `git show`).
    /// The commit is immutable, so the diff is captured once and not
    /// refreshed on an interval.
    #[clap(
        short = 'c',
        long = "change",
        conflicts_with = "rev",
        conflicts_with = "origin_pr",
        conflicts_with = "github_pr"
    )]
    commit: Option<String>,

    /// Show Origin pull request N (`origin pr diff N`). Fetched once;
    /// not refreshed on an interval.
    #[clap(
        long,
        conflicts_with = "github_pr",
        conflicts_with = "rev",
        conflicts_with = "commit",
        conflicts_with = "sl",
        conflicts_with = "git"
    )]
    origin_pr: Option<u64>,

    /// Show GitHub pull request N (`gh pr diff N`). Fetched once;
    /// not refreshed on an interval.
    #[clap(
        long,
        conflicts_with = "origin_pr",
        conflicts_with = "rev",
        conflicts_with = "commit",
        conflicts_with = "sl",
        conflicts_with = "git"
    )]
    github_pr: Option<u64>,

    /// Initial unified-diff context lines. `u` / `U` change it later.
    #[clap(short = 'U', long = "unified", default_value_t = DEFAULT_CONTEXT)]
    context: u32,

    /// Open the diff stat view.
    #[clap(long, conflicts_with = "tree")]
    stat: bool,

    /// Open the smartlog view. Requires a Sapling repository.
    #[clap(long, conflicts_with = "stat", conflicts_with = "git")]
    tree: bool,
}

fn install_panic_hook() {
    let original = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        restore_terminal();
        original(info);
    }));
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let spec = capture_spec(&args)?;
    // `d` can switch to a working-copy mode; that needs the repo tool.
    // A PR diff may run outside a repository, in which case the menu
    // cannot switch.
    let menu_tool = match &spec {
        CaptureSpec::Live { tool, .. } => Some(*tool),
        CaptureSpec::Commit { tool, .. } => Some(*tool),
        CaptureSpec::Fixed { .. } => diff_tool().ok(),
    };
    let start_view = if args.tree {
        match menu_tool {
            Some(DiffTool::Sl) => StartView::Tree,
            Some(DiffTool::Git) => anyhow::bail!("--tree requires Sapling"),
            None => anyhow::bail!("--tree requires a Sapling repository"),
        }
    } else if args.stat {
        StartView::Stat
    } else {
        StartView::Diff
    };
    let spec = Arc::new(Mutex::new(spec));
    let context = Arc::new(AtomicU32::new(args.context.min(MAX_CONTEXT)));
    let history = History::load_from_home().await;

    // Capture the first snapshot on the primary screen so the alternate
    // screen is not shown empty while the command runs.
    let (output_tx, mut output_rx) = mpsc::channel(1);
    let (refresh_tx, refresh_rx) = NotifyTx::pair();
    let capture_task = AbortOnDropHandle::spawn(capture_command_output_loop(
        Arc::clone(&spec),
        Arc::clone(&context),
        args.interval.as_duration(),
        output_tx,
        refresh_rx,
    ));
    let Some(first_lines) = output_rx.recv().await else {
        capture_task.await?;
        return Ok(());
    };

    install_panic_hook();
    // Query before raw mode; the color query reads a terminal response.
    let terminal_bg = query_terminal_bg();
    let _guard = TerminalGuard::enter_alt_mode()?;
    let size = terminal_size()?;

    // One reader thread: crossterm events must not be polled concurrently.
    // `poll` with a timeout lets the thread exit after `stop` is set.
    let stop = Arc::new(AtomicBool::new(false));
    let _stop_on_drop = StopOnDrop(Arc::clone(&stop));
    let (_key_reader, keys) = spawn_key_reader(stop);

    EventLoop::new(
        terminal_bg,
        history,
        size,
        context,
        spec,
        menu_tool,
        start_view,
    )
    .run(first_lines, output_rx, keys, refresh_tx, capture_task)
    .await
}

/// PR diffs do not need a local repository. A working-copy or commit
/// diff does, and that is when the sl/git tool is resolved.
fn capture_spec(args: &Args) -> anyhow::Result<CaptureSpec> {
    if let Some(number) = args.origin_pr {
        return Ok(CaptureSpec::Fixed {
            command: origin_pr_diff_command(number),
        });
    }
    if let Some(number) = args.github_pr {
        return Ok(CaptureSpec::Fixed {
            command: github_pr_diff_command(number),
        });
    }
    let tool = if args.sl {
        DiffTool::Sl
    } else if args.git {
        DiffTool::Git
    } else {
        diff_tool()?
    };
    if let Some(commit) = &args.commit {
        return Ok(CaptureSpec::Commit {
            tool,
            commit: commit.clone(),
        });
    }
    Ok(CaptureSpec::Live {
        tool,
        rev: args.rev.clone(),
    })
}

fn spawn_key_reader(
    stop: Arc<AtomicBool>,
) -> (AbortOnDropHandle<()>, UnboundedReceiver<io::Result<Event>>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let handle = AbortOnDropHandle::spawn_blocking(move || {
        while !stop.load(Ordering::SeqCst) {
            match crossterm::event::poll(Duration::from_millis(50)) {
                Ok(false) => {}
                Ok(true) => match crossterm::event::read() {
                    Ok(event) => {
                        if tx.send(Ok(event)).is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        let _ = tx.send(Err(err));
                        break;
                    }
                },
                Err(err) => {
                    let _ = tx.send(Err(err));
                    break;
                }
            }
        }
    });
    (handle, rx)
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use crate::Args;

    #[test]
    fn test_sl_and_git_flags_conflict() {
        let err = Args::try_parse_from(["sl-tui", "--sl", "--git"]).unwrap_err();
        let text = err.to_string();
        assert_eq!(true, text.contains("--sl"));
        assert_eq!(true, text.contains("--git"));
    }

    #[test]
    fn test_sl_flag_parses() {
        let args = Args::try_parse_from(["sl-tui", "--sl"]).unwrap();
        assert_eq!(true, args.sl);
        assert_eq!(false, args.git);
    }

    #[test]
    fn test_git_flag_parses() {
        let args = Args::try_parse_from(["sl-tui", "--git"]).unwrap();
        assert_eq!(false, args.sl);
        assert_eq!(true, args.git);
    }

    #[test]
    fn test_rev_flag_parses() {
        let args = Args::try_parse_from(["sl-tui", "-r", ".^"]).unwrap();
        assert_eq!(Some(String::from(".^")), args.rev);
        let args = Args::try_parse_from(["sl-tui", "--rev", "main"]).unwrap();
        assert_eq!(Some(String::from("main")), args.rev);
        let args = Args::try_parse_from(["sl-tui"]).unwrap();
        assert_eq!(None, args.rev);
    }

    #[test]
    fn test_origin_pr_flag_parses() {
        let args = Args::try_parse_from(["sl-tui", "--origin-pr", "41"]).unwrap();
        assert_eq!(Some(41), args.origin_pr);
        assert_eq!(None, args.github_pr);
        let spec = crate::capture_spec(&args).unwrap();
        match spec {
            crate::capture::CaptureSpec::Fixed { command } => {
                assert_eq!(crate::pr_diff::origin_pr_diff_command(41), command);
            }
            crate::capture::CaptureSpec::Live { .. } => panic!("expected fixed command"),
            crate::capture::CaptureSpec::Commit { .. } => panic!("expected fixed command"),
        }
    }

    #[test]
    fn test_github_pr_flag_parses() {
        let args = Args::try_parse_from(["sl-tui", "--github-pr", "41"]).unwrap();
        assert_eq!(Some(41), args.github_pr);
        assert_eq!(None, args.origin_pr);
        let spec = crate::capture_spec(&args).unwrap();
        match spec {
            crate::capture::CaptureSpec::Fixed { command } => {
                assert_eq!(crate::pr_diff::github_pr_diff_command(41), command);
            }
            crate::capture::CaptureSpec::Live { .. } => panic!("expected fixed command"),
            crate::capture::CaptureSpec::Commit { .. } => panic!("expected fixed command"),
        }
    }

    #[test]
    fn test_pr_flags_conflict_with_each_other_and_rev() {
        let err =
            Args::try_parse_from(["sl-tui", "--origin-pr", "1", "--github-pr", "2"]).unwrap_err();
        assert_eq!(true, err.to_string().contains("--origin-pr"));
        let err = Args::try_parse_from(["sl-tui", "--origin-pr", "1", "-r", "main"]).unwrap_err();
        assert_eq!(true, err.to_string().contains("--origin-pr"));
        let err = Args::try_parse_from(["sl-tui", "--github-pr", "1", "-c", "abc"]).unwrap_err();
        assert_eq!(true, err.to_string().contains("--github-pr"));
        let err = Args::try_parse_from(["sl-tui", "--origin-pr", "1", "--sl"]).unwrap_err();
        assert_eq!(true, err.to_string().contains("--origin-pr"));
    }

    #[test]
    fn test_unified_flag_parses() {
        let args = Args::try_parse_from(["sl-tui"]).unwrap();
        assert_eq!(crate::repo::repo::DEFAULT_CONTEXT, args.context);
        let args = Args::try_parse_from(["sl-tui", "-U", "0"]).unwrap();
        assert_eq!(0, args.context);
        let args = Args::try_parse_from(["sl-tui", "--unified", "12"]).unwrap();
        assert_eq!(12, args.context);
    }

    #[test]
    fn test_stat_and_tree_flags_parse() {
        let args = Args::try_parse_from(["sl-tui"]).unwrap();
        assert_eq!(false, args.stat);
        assert_eq!(false, args.tree);
        let args = Args::try_parse_from(["sl-tui", "--stat"]).unwrap();
        assert_eq!(true, args.stat);
        assert_eq!(false, args.tree);
        let args = Args::try_parse_from(["sl-tui", "--tree"]).unwrap();
        assert_eq!(false, args.stat);
        assert_eq!(true, args.tree);
        let err = Args::try_parse_from(["sl-tui", "--stat", "--tree"]).unwrap_err();
        assert_eq!(true, err.to_string().contains("--stat"));
        let err = Args::try_parse_from(["sl-tui", "--tree", "--git"]).unwrap_err();
        assert_eq!(true, err.to_string().contains("--tree"));
    }
}

/// Sets the key-reader stop flag when the watch loop exits.
struct StopOnDrop(Arc<AtomicBool>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
