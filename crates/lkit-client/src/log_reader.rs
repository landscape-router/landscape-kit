//! File-based log reader — reads Landscape log files from disk.

use std::path::PathBuf;

use async_trait::async_trait;

use lkit_core::{CoreError, LogReader};

/// Reads the most recent log file from a directory.
pub struct FileLogReader {
    logs_dir: PathBuf,
}

impl FileLogReader {
    /// Create a reader that scans `logs_dir` for log files.
    pub fn new(logs_dir: PathBuf) -> Self {
        Self { logs_dir }
    }
}

#[async_trait]
impl LogReader for FileLogReader {
    /// Read the last `lines` lines from the most recent log file in `logs_dir`.
    ///
    /// Files are sorted by name (descending) to pick the latest.
    /// Returns an empty vec if no log files exist.
    async fn recent_lines(&self, lines: usize) -> Result<Vec<String>, CoreError> {
        let mut entries = Vec::new();
        let mut dir = tokio::fs::read_dir(&self.logs_dir).await?;

        while let Some(entry) = dir.next_entry().await? {
            let path = entry.path();
            if path.is_file() {
                entries.push(path);
            }
        }

        // Sort descending by file name — latest first.
        entries.sort_by(|a, b| b.file_name().cmp(&a.file_name()));

        let latest = match entries.first() {
            Some(p) => p,
            None => return Ok(vec![]),
        };

        let content = tokio::fs::read_to_string(latest).await?;

        let all_lines: Vec<String> = content.lines().map(String::from).collect();
        let start = all_lines.len().saturating_sub(lines);
        Ok(all_lines[start..].to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_log_reader_stores_dir() {
        let reader = FileLogReader::new(PathBuf::from("/tmp/logs"));
        assert_eq!(reader.logs_dir, PathBuf::from("/tmp/logs"));
    }

    #[tokio::test]
    async fn recent_lines_returns_empty_when_no_files() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let reader = FileLogReader::new(dir.path().to_path_buf());
        let result = reader.recent_lines(10).await?;
        assert!(result.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn recent_lines_reads_latest_file() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        tokio::fs::write(
            dir.path().join("app.log.20260101"),
            "old line 1\nold line 2\n",
        )
        .await?;
        tokio::fs::write(
            dir.path().join("app.log.20260102"),
            "new line 1\nnew line 2\nnew line 3\n",
        )
        .await?;

        let reader = FileLogReader::new(dir.path().to_path_buf());
        let result = reader.recent_lines(2).await?;
        assert_eq!(result, vec!["new line 2", "new line 3"]);
        Ok(())
    }

    #[tokio::test]
    async fn recent_lines_limits_to_n() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        tokio::fs::write(
            dir.path().join("app.log"),
            "line1\nline2\nline3\nline4\nline5\n",
        )
        .await?;

        let reader = FileLogReader::new(dir.path().to_path_buf());
        let result = reader.recent_lines(3).await?;
        assert_eq!(result, vec!["line3", "line4", "line5"]);
        Ok(())
    }
}
