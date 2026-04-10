use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result, bail};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;
use zip::CompressionMethod;
use zip::write::SimpleFileOptions;

static LOG_GUARD: OnceLock<WorkerGuard> = OnceLock::new();

pub fn initialize_logging(log_dir: &Path) -> Result<()> {
    if LOG_GUARD.get().is_some() {
        return Ok(());
    }

    fs::create_dir_all(log_dir)
        .with_context(|| format!("failed to create log directory: {}", log_dir.display()))?;
    let file_appender = tracing_appender::rolling::daily(log_dir, "oci-sniper.log");
    let (writer, guard) = tracing_appender::non_blocking(file_appender);
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(writer)
        .with_ansi(false)
        .finish();

    tracing::subscriber::set_global_default(subscriber)
        .context("failed to set global tracing subscriber")?;
    let _ = LOG_GUARD.set(guard);
    Ok(())
}

pub fn zip_logs(log_dir: &Path, limit: Option<usize>) -> Result<PathBuf> {
    let log_files = collect_log_files(log_dir, limit)?;
    if log_files.is_empty() {
        bail!("no log files found in {}", log_dir.display());
    }

    let archive_path = std::env::temp_dir().join(format!(
        "oci-sniper-logs-{}.zip",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let archive = File::create(&archive_path)
        .with_context(|| format!("failed to create log archive: {}", archive_path.display()))?;
    let mut zip = zip::ZipWriter::new(archive);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    for file_path in log_files {
        let file_name = file_path
            .file_name()
            .and_then(|name| name.to_str())
            .context("log file name is not valid UTF-8")?;
        zip.start_file(file_name, options)
            .context("failed to start zip entry")?;
        let mut contents = Vec::new();
        File::open(&file_path)
            .with_context(|| format!("failed to open log file: {}", file_path.display()))?
            .read_to_end(&mut contents)
            .with_context(|| format!("failed to read log file: {}", file_path.display()))?;
        zip.write_all(&contents)
            .with_context(|| format!("failed to write zip entry for {}", file_path.display()))?;
    }

    zip.finish().context("failed to finalize log archive")?;
    Ok(archive_path)
}

pub fn latest_log_tail(
    log_dir: &Path,
    line_limit: usize,
    max_chars: usize,
) -> Result<Option<String>> {
    let Some(path) = newest_log_file(log_dir)? else {
        return Ok(None);
    };
    let contents = fs::read_to_string(&path)
        .with_context(|| format!("failed to read latest log file: {}", path.display()))?;
    let tail_lines = contents
        .lines()
        .rev()
        .take(line_limit.max(1))
        .collect::<Vec<_>>();
    let tail = tail_lines.into_iter().rev().collect::<Vec<_>>().join("\n");

    if tail.chars().count() <= max_chars {
        return Ok(Some(tail));
    }

    let truncated = tail
        .chars()
        .rev()
        .take(max_chars)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    Ok(Some(truncated))
}

fn collect_log_files(log_dir: &Path, limit: Option<usize>) -> Result<Vec<PathBuf>> {
    let mut files = read_log_entries(log_dir)?;
    files.sort_by(|left, right| right.1.cmp(&left.1));
    Ok(files
        .into_iter()
        .take(limit.unwrap_or(usize::MAX))
        .map(|(path, _)| path)
        .collect())
}

fn newest_log_file(log_dir: &Path) -> Result<Option<PathBuf>> {
    Ok(read_log_entries(log_dir)?
        .into_iter()
        .max_by(|left, right| left.1.cmp(&right.1))
        .map(|(path, _)| path))
}

fn read_log_entries(log_dir: &Path) -> Result<Vec<(PathBuf, std::time::SystemTime)>> {
    if !log_dir.exists() {
        return Ok(Vec::new());
    }

    let mut entries = Vec::new();
    for entry in fs::read_dir(log_dir)
        .with_context(|| format!("failed to read log directory: {}", log_dir.display()))?
    {
        let entry = entry.context("failed to read log directory entry")?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let modified = entry
            .metadata()
            .with_context(|| format!("failed to read metadata for {}", path.display()))?
            .modified()
            .with_context(|| format!("failed to read mtime for {}", path.display()))?;
        entries.push((path, modified));
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn zips_only_requested_number_of_latest_logs() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.log"), "a").unwrap();
        thread::sleep(Duration::from_millis(10));
        fs::write(dir.path().join("b.log"), "b").unwrap();

        let archive_path = zip_logs(dir.path(), Some(1)).unwrap();
        let archive = File::open(archive_path).unwrap();
        let mut zip = zip::ZipArchive::new(archive).unwrap();

        assert_eq!(zip.len(), 1);
        assert_eq!(zip.by_index(0).unwrap().name(), "b.log");
    }

    #[test]
    fn returns_truncated_tail_from_latest_log() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("latest.log"),
            "line1\nline2\nline3\nline4\nline5\nline6",
        )
        .unwrap();

        let tail = latest_log_tail(dir.path(), 4, 10).unwrap().unwrap();

        assert_eq!(tail, "ine5\nline6");
    }
}
