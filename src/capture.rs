use std::io;
use std::io::ErrorKind;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::fd::OwnedFd;
use std::process::Stdio;
use std::time::Duration;

use anyhow::anyhow;
use tokio::net::unix::pipe;
use tokio::process::Child;
use tokio::sync::mpsc::Sender;

use crate::diff::DiffOutput;
use crate::diff::parse_diff;
use crate::notify_pair::NotifyRx;

/// Result of a capture that may be cut short by `interrupt`.
pub(crate) enum CaptureOutcome {
    /// Child exited (or the pipe hit EOF) and output was collected.
    Done(Vec<u8>),
    /// `interrupt` fired. The child and its process group were killed.
    Interrupted,
}

/// Run `command` and return its combined stdout and stderr bytes.
/// A notification on `interrupt` kills the child and returns
/// [`CaptureOutcome::Interrupted`].
pub(crate) async fn capture_command_output_interruptible(
    command: &[String],
    interrupt: Option<&NotifyRx>,
) -> anyhow::Result<CaptureOutcome> {
    let Some((program, args)) = command.split_first() else {
        return Err(anyhow!("empty command"));
    };
    // One pipe for both streams: the kernel keeps write order.
    let (reader, writer) = io::pipe()?;
    let stderr_writer = writer.try_clone()?;
    // Own process group so an interrupt can kill grandchildren too.
    let mut child = tokio::process::Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(writer)
        .stderr(stderr_writer)
        .kill_on_drop(true)
        .process_group(0)
        .spawn()?;
    // Tokio drives the read end. The child keeps a blocking write end.
    let reader = pipe::Receiver::from_owned_fd(OwnedFd::from(reader))?;
    let mut out = Vec::new();
    let mut buf = [0_u8; 8192];
    loop {
        tokio::select! {
            biased;
            () = wait_interrupt(interrupt) => {
                kill_process_group(&mut child).await;
                return Ok(CaptureOutcome::Interrupted);
            }
            ready = reader.readable() => {
                ready?;
                match read_ready(&reader, &mut buf, &mut out)? {
                    ReadReady::Eof => {
                        let status = child.wait().await?;
                        append_nonzero_status(&status, &mut out);
                        return Ok(CaptureOutcome::Done(out));
                    }
                    ReadReady::Pending => {}
                }
            }
            status = child.wait() => {
                let status = status?;
                // Do not wait for EOF: a grandchild may still hold the write end.
                // Drop the tokio registration and read whatever is already buffered.
                drain_nonblocking(reader.into_nonblocking_fd()?, &mut out)?;
                append_nonzero_status(&status, &mut out);
                return Ok(CaptureOutcome::Done(out));
            }
        }
    }
}

/// Capture in a loop, sending each snapshot on `tx`.
/// `refresh` kills the current command and starts the next run at once.
pub(crate) async fn capture_command_output_loop(
    command: Vec<String>,
    interval: Duration,
    tx: Sender<DiffOutput>,
    refresh: NotifyRx,
) {
    loop {
        let outcome = capture_command_output_interruptible(&command, Some(&refresh)).await;
        let output = match outcome {
            Ok(CaptureOutcome::Interrupted) => continue,
            Ok(CaptureOutcome::Done(bytes)) => parse_diff(&bytes),
            Err(err) => {
                let text = format!("error: {err:?}");
                DiffOutput::message(&text)
            }
        };
        if tx.send(output).await.is_err() {
            return;
        }
        tokio::select! {
            () = tokio::time::sleep(interval) => {}
            () = refresh.notified() => {}
        }
    }
}

async fn wait_interrupt(interrupt: Option<&NotifyRx>) {
    match interrupt {
        Some(rx) => rx.notified().await,
        None => std::future::pending().await,
    }
}

/// SIGKILL the child's process group, then reap the child.
async fn kill_process_group(child: &mut Child) {
    if let Some(id) = child.id() {
        // SAFETY: `id` is the pid of a child we spawned with `process_group(0)`,
        // so it is the process-group id. Negative pid targets that group.
        unsafe {
            libc::kill(-(id as i32), libc::SIGKILL);
        }
    }
    let _ = child.start_kill();
    let _ = child.wait().await;
}

enum ReadReady {
    Eof,
    Pending,
}

/// Read while tokio still considers the pipe readable.
fn read_ready(
    reader: &pipe::Receiver,
    buf: &mut [u8],
    out: &mut Vec<u8>,
) -> anyhow::Result<ReadReady> {
    loop {
        match reader.try_read(buf) {
            Ok(0) => return Ok(ReadReady::Eof),
            Ok(n) => out.extend_from_slice(&buf[..n]),
            Err(err) if err.kind() == ErrorKind::WouldBlock => return Ok(ReadReady::Pending),
            Err(err) => return Err(err.into()),
        }
    }
}

/// Append bytes already buffered on `reader`. Stop at EOF or when a read would block.
fn drain_nonblocking(fd: OwnedFd, out: &mut Vec<u8>) -> anyhow::Result<()> {
    let mut reader = io::PipeReader::from(fd);
    set_nonblocking(&reader)?;
    let mut buf = [0_u8; 8192];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => out.extend_from_slice(&buf[..n]),
            Err(err) if err.kind() == ErrorKind::Interrupted => {}
            Err(err) if err.kind() == ErrorKind::WouldBlock => return Ok(()),
            Err(err) => return Err(err.into()),
        }
    }
}

fn append_nonzero_status(status: &std::process::ExitStatus, out: &mut Vec<u8>) {
    if status.success() {
        return;
    }
    if !out.is_empty() && !out.ends_with(b"\n") {
        out.push(b'\n');
    }
    let line = match status.code() {
        Some(code) => format!("exit status: {code}\n"),
        None => format!("{status}\n"),
    };
    out.extend_from_slice(line.as_bytes());
}

fn set_nonblocking(fd: &impl AsRawFd) -> anyhow::Result<()> {
    let fd = fd.as_raw_fd();
    // SAFETY: `fd` is an open file descriptor we own for the length of the call.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error().into());
    }
    // SAFETY: same fd; F_SETFL with the existing flags plus O_NONBLOCK.
    let rc = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if rc < 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Write;
    use std::os::fd::OwnedFd;
    use std::path::PathBuf;
    use std::time::Duration;

    use crate::capture::CaptureOutcome;
    use crate::capture::capture_command_output_interruptible;
    use crate::capture::drain_nonblocking;

    async fn capture_command_output(command: &[String]) -> anyhow::Result<Vec<u8>> {
        match capture_command_output_interruptible(command, None).await? {
            CaptureOutcome::Done(bytes) => Ok(bytes),
            CaptureOutcome::Interrupted => Err(anyhow::anyhow!("capture interrupted")),
        }
    }

    #[test]
    fn test_drain_nonblocking_reads_buffer_and_stops_with_writer_open() {
        let (reader, mut writer) = std::io::pipe().unwrap();
        writer.write_all(b"hello").unwrap();
        let mut out = Vec::new();
        out.extend_from_slice(b"pre");
        // Writer stays open, so a blocking read would wait for EOF.
        drain_nonblocking(OwnedFd::from(reader), &mut out).unwrap();
        assert_eq!(b"prehello".as_slice(), out.as_slice());
        drop(writer);
    }

    #[test]
    fn test_drain_nonblocking_empty_pipe_with_writer_open() {
        let (reader, writer) = std::io::pipe().unwrap();
        let mut out = Vec::new();
        drain_nonblocking(OwnedFd::from(reader), &mut out).unwrap();
        assert_eq!(Vec::<u8>::new(), out);
        drop(writer);
    }

    #[test]
    fn test_drain_nonblocking_reads_to_eof() {
        let (reader, mut writer) = std::io::pipe().unwrap();
        writer.write_all(b"abc").unwrap();
        drop(writer);
        let mut out = Vec::new();
        drain_nonblocking(OwnedFd::from(reader), &mut out).unwrap();
        assert_eq!(b"abc".as_slice(), out.as_slice());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_capture_stdout_and_stderr() {
        let command = [
            String::from("sh"),
            String::from("-c"),
            String::from("printf 'a\\n'; printf 'b\\n' >&2"),
        ];
        let bytes = capture_command_output(&command).await.unwrap();
        assert_eq!(b"a\nb\n".as_slice(), bytes.as_slice());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_capture_appends_nonzero_exit_status() {
        let command = [
            String::from("sh"),
            String::from("-c"),
            String::from("printf 'no\\n'; exit 3"),
        ];
        let bytes = capture_command_output(&command).await.unwrap();
        assert_eq!(b"no\nexit status: 3\n".as_slice(), bytes.as_slice());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_interrupt_kills_hung_command() {
        use crate::notify_pair::NotifyTx;

        let (tx, rx) = NotifyTx::pair();
        let command = [String::from("sleep"), String::from("30")];
        let capture = capture_command_output_interruptible(&command, Some(&rx));
        let interrupter = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            tx.notify_one();
        };
        let joined = tokio::time::timeout(Duration::from_secs(2), async {
            tokio::join!(capture, interrupter)
        });
        let (outcome, ()) = joined.await.expect("interrupt did not kill the child");
        assert_eq!(
            true,
            matches!(outcome.unwrap(), CaptureOutcome::Interrupted)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_capture_returns_when_child_exits_despite_open_pipe() {
        let pid_path = PathBuf::from(format!(
            "/tmp/live-diff-capture-test-{}.pid",
            std::process::id()
        ));
        let _cleanup = KillPidFile {
            path: pid_path.clone(),
        };
        let script = format!(
            "printf 'hi\\n'; sleep 30 & echo $! > '{}'",
            pid_path.display()
        );
        let command = [String::from("sh"), String::from("-c"), script];
        let bytes = tokio::time::timeout(Duration::from_secs(2), capture_command_output(&command))
            .await
            .expect("capture waited for the grandchild")
            .unwrap();
        assert_eq!(b"hi\n".as_slice(), bytes.as_slice());
    }

    struct KillPidFile {
        path: PathBuf,
    }

    impl Drop for KillPidFile {
        fn drop(&mut self) {
            if let Ok(text) = fs::read_to_string(&self.path) {
                if let Ok(pid) = text.trim().parse::<i32>() {
                    // SAFETY: pid came from a child we started; SIGKILL is fine in tests.
                    unsafe {
                        libc::kill(pid, libc::SIGKILL);
                    }
                }
            }
            let _ = fs::remove_file(&self.path);
        }
    }
}
