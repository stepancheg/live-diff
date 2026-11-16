use std::env;
use std::mem;
use std::path::PathBuf;

use crate::search::history::file::HistoryFile;
use crate::search::history::file::TRUNCATE_ABOVE;
use crate::search::history::file::TRUNCATE_TO;

const HISTORY_FILE: &str = ".live-diff.history";

pub(crate) struct History {
    entries: Vec<String>,
    file: Option<HistoryFile>,
}

impl History {
    pub(crate) async fn load_from_home() -> History {
        match history_path() {
            Some(path) => History::load(path).await,
            None => History {
                entries: Vec::new(),
                file: None,
            },
        }
    }

    async fn load(path: PathBuf) -> History {
        let (file, entries) = HistoryFile::start(path).await;
        History { entries, file }
    }

    pub(crate) fn entries(&self) -> &[String] {
        &self.entries
    }

    pub(crate) fn record(&mut self, entry: &str) {
        if entry.is_empty() {
            return;
        }
        if self.entries.last().is_some_and(|last| last == entry) {
            return;
        }
        self.entries.push(entry.to_owned());
        if self.entries.len() > TRUNCATE_ABOVE {
            let start = self.entries.len() - TRUNCATE_TO;
            self.entries.drain(..start);
        }
        if let Some(file) = &self.file {
            file.send(entry);
        }
    }

    /// Drop the sender and wait until queued writes have been applied.
    pub(crate) async fn shutdown(&mut self) {
        if let Some(file) = mem::take(&mut self.file) {
            file.shutdown().await;
        }
    }
}

fn history_path() -> Option<PathBuf> {
    let home = env::var_os("HOME")?;
    if home.is_empty() {
        return None;
    }
    let mut path = PathBuf::from(home);
    path.push(HISTORY_FILE);
    Some(path)
}

#[cfg(test)]
mod tests {
    use crate::search::history::store::History;

    #[tokio::test(flavor = "current_thread")]
    async fn test_record_appends_lines() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        let mut history = History::load(path.clone()).await;
        history.record("alpha");
        history.record("beta");
        history.record("");
        history.record("beta");
        assert_eq!(vec_of("alpha", "beta"), history.entries().to_vec());
        history.shutdown().await;
        let reloaded = History::load(path).await;
        assert_eq!(vec_of("alpha", "beta"), reloaded.entries().to_vec());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_record_truncates_to_1000_when_above_2000() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        let mut history = History::load(path.clone()).await;
        for i in 0..2001 {
            history.record(&format!("p{i}"));
        }
        assert_eq!(1000, history.entries().len());
        assert_eq!("p1001", history.entries()[0]);
        assert_eq!("p2000", history.entries()[999]);
        history.shutdown().await;
        let reloaded = History::load(path).await;
        assert_eq!(1000, reloaded.entries().len());
        assert_eq!("p1001", reloaded.entries()[0]);
        assert_eq!("p2000", reloaded.entries()[999]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_record_sends_append_to_writer() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        let mut history = History::load(path.clone()).await;
        history.record("alpha");
        history.record("beta");
        history.shutdown().await;
        let reloaded = History::load(path).await;
        assert_eq!(vec_of("alpha", "beta"), reloaded.entries().to_vec());
    }

    fn vec_of(a: &str, b: &str) -> Vec<String> {
        let mut entries = Vec::new();
        entries.push(a.to_owned());
        entries.push(b.to_owned());
        entries
    }
}
