use std::io;
use std::path::Path;
use std::path::PathBuf;

use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncSeekExt;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::UnboundedSender;

use crate::abort_on_drop_handle::AbortOnDropHandle;

pub(crate) const TRUNCATE_ABOVE: usize = 2000;
pub(crate) const TRUNCATE_TO: usize = 1000;

/// On-disk history: the append channel and the task that drains it.
/// These only exist together.
pub(crate) struct HistoryFile {
    writes: UnboundedSender<String>,
    writer: AbortOnDropHandle<()>,
}

impl HistoryFile {
    pub(crate) async fn start(path: PathBuf) -> (Option<HistoryFile>, Vec<String>) {
        let Ok(mut file) = open_append(&path).await else {
            return (None, Vec::new());
        };
        let entries = read_entries_from_file(&mut file)
            .await
            .unwrap_or_else(|_| Vec::new());
        let line_count = entries.len();
        let (writes, rx) = tokio::sync::mpsc::unbounded_channel();
        let writer = AbortOnDropHandle::spawn(run_writer(rx, file, line_count));
        (Some(HistoryFile { writes, writer }), entries)
    }

    pub(crate) fn send(&self, entry: &str) {
        let _ = self.writes.send(entry.to_owned());
    }

    pub(crate) async fn shutdown(self) {
        let HistoryFile { writes, writer } = self;
        drop(writes);
        let _ = writer.await;
    }
}

async fn run_writer(mut rx: UnboundedReceiver<String>, mut file: File, mut line_count: usize) {
    while let Some(line) = rx.recv().await {
        let buf = format!("{line}\n");
        if file.write_all(buf.as_bytes()).await.is_err() {
            return;
        }
        line_count += 1;
        if line_count > TRUNCATE_ABOVE {
            match truncate_file_to(&mut file, TRUNCATE_ABOVE, TRUNCATE_TO).await {
                Ok(n) => line_count = n,
                Err(_) => return,
            }
        }
    }
}

async fn truncate_file_to(file: &mut File, above: usize, keep: usize) -> io::Result<usize> {
    let entries = read_entries_from_file(file).await?;
    // Another process may already have shortened the file.
    if entries.len() <= above {
        return Ok(entries.len());
    }
    let mut out = String::new();
    let mut kept = 0;
    for entry in entries.iter().rev().take(keep).rev() {
        out.push_str(entry);
        out.push('\n');
        kept += 1;
    }
    file.set_len(0).await?;
    file.rewind().await?;
    file.write_all(out.as_bytes()).await?;
    Ok(kept)
}

async fn read_entries_from_file(file: &mut File) -> io::Result<Vec<String>> {
    file.rewind().await?;
    let mut text = String::new();
    file.read_to_string(&mut text).await?;
    Ok(parse_entries(&text))
}

fn parse_entries(text: &str) -> Vec<String> {
    let mut entries = Vec::new();
    for line in text.lines() {
        if !line.is_empty() {
            entries.push(line.to_owned());
        }
    }
    entries
}

async fn open_append(path: &Path) -> io::Result<File> {
    tokio::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(path)
        .await
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tokio::io::AsyncWriteExt;

    use crate::search::history::file::open_append;
    use crate::search::history::file::truncate_file_to;

    #[tokio::test(flavor = "current_thread")]
    async fn test_truncate_file_to() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        fs::write(&path, "a\nb\nc\nd\n").unwrap();
        let mut file = open_append(&path).await.unwrap();
        assert_eq!(2, truncate_file_to(&mut file, 2, 2).await.unwrap());
        file.write_all(b"e\n").await.unwrap();
        drop(file);
        assert_eq!("c\nd\ne\n", fs::read_to_string(&path).unwrap());

        let mut file = open_append(&path).await.unwrap();
        assert_eq!(3, truncate_file_to(&mut file, 10, 2).await.unwrap());
        drop(file);
        assert_eq!("c\nd\ne\n", fs::read_to_string(&path).unwrap());
    }
}
