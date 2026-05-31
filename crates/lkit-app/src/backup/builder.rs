//! tar.gz packaging: create and inspect backup archives.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use tar::Header;

use crate::AppError;

/// Format the current system time as `YYYYMMDD-HHMMSS`.
pub fn timestamp() -> String {
    let dur = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = dur.as_secs();
    // days since epoch
    let days = secs / 86400;
    // time within day
    let time = secs % 86400;
    let hours = time / 3600;
    let minutes = (time % 3600) / 60;
    let seconds = time % 60;

    // A simple leap-year-aware date calculation (valid 1970-2100).
    let mut y = 1970i64;
    let mut remaining = days as i64;
    loop {
        let days_in_year = if is_leap(y) { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        y += 1;
    }
    let month_days = if is_leap(y) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut m = 0usize;
    for (i, &md) in month_days.iter().enumerate() {
        if remaining < md as i64 {
            m = i;
            break;
        }
        remaining -= md as i64;
    }
    let d = remaining + 1;
    format!("{:04}{:02}{:02}-{:02}{:02}{:02}", y, m + 1, d, hours, minutes, seconds)
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// Generate an RFC 3339 timestamp string from the current system time.
pub fn rfc3339() -> String {
    let dur = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = dur.as_secs();
    let days = secs / 86400;
    let time = secs % 86400;
    let hours = time / 3600;
    let minutes = (time % 3600) / 60;
    let seconds = time % 60;

    let mut y = 1970i64;
    let mut remaining = days as i64;
    loop {
        let days_in_year = if is_leap(y) { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        y += 1;
    }
    let month_days = if is_leap(y) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut m = 0usize;
    for (i, &md) in month_days.iter().enumerate() {
        if remaining < md as i64 {
            m = i;
            break;
        }
        remaining -= md as i64;
    }
    let d = remaining + 1;
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, m + 1, d, hours, minutes, seconds)
}

/// Package the contents of `staging_dir` into a tar.gz archive at `output_path`.
///
/// Uses gzip level 6. The binary file (detected by the `landscape-webserver`
/// basename) is stored with 0o755 permissions; all other files use 0o644.
/// The output file is created with 0o600 permissions.
pub fn build_package(staging_dir: &Path, output_path: &Path) -> Result<(), AppError> {
    use flate2::Compression;
    use flate2::write::GzEncoder;

    let file = std::fs::File::create(output_path)?;
    let encoder = GzEncoder::new(file, Compression::new(6));
    let mut archive = tar::Builder::new(encoder);

    add_dir_entries(&mut archive, staging_dir, staging_dir)?;

    let encoder = archive.into_inner()?;
    encoder.finish()?;

    // Set restrictive permissions on the archive.
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(output_path, std::fs::Permissions::from_mode(0o600))?;

    Ok(())
}

fn add_dir_entries(
    archive: &mut tar::Builder<flate2::write::GzEncoder<std::fs::File>>,
    base: &Path,
    dir: &Path,
) -> Result<(), AppError> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let relative = path
            .strip_prefix(base)
            .map_err(|e| AppError::Backup(format!("path prefix error: {e}")))?;

        if path.is_dir() {
            archive.append_dir(relative, &path)?;

            add_dir_entries(archive, base, &path)?;
        } else if path.is_file() {
            let is_binary = relative
                .file_name()
                .and_then(|f| f.to_str())
                .is_some_and(|f| f == "landscape-webserver");

            let mut header = Header::new_ustar();
            let metadata = std::fs::metadata(&path)?;
            header.set_size(metadata.len());
            header.set_mtime(
                metadata
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
            );
            if is_binary {
                header.set_mode(0o755);
            } else {
                header.set_mode(0o644);
            }
            header.set_entry_type(tar::EntryType::Regular);
            header.set_username("root")?;
            header.set_groupname("root")?;
            header.set_cksum();

            let data = std::fs::read(&path)?;
            archive.append_data(&mut header, relative, data.as_slice())?;
        }
    }
    Ok(())
}

/// Compute the SHA-256 hex digest of a file.
pub fn sha256_file(path: &Path) -> Result<String, AppError> {
    use sha2::{Digest, Sha256};
    let data = std::fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&data);
    Ok(format!("{:x}", hasher.finalize()))
}

/// Compute the SHA-256 hex digest of in-memory data.
pub fn sha256_data(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::GzDecoder;
    use std::io::Read;
    use tempfile::TempDir;

    #[test]
    fn build_package_creates_valid_tar_gz() {
        let staging = TempDir::new().unwrap();
        let output_dir = TempDir::new().unwrap();
        let output = output_dir.path().join("backup.tar.gz");

        // Create test files in staging.
        std::fs::write(staging.path().join("landscape-webserver"), "binary-content").unwrap();
        std::fs::write(staging.path().join("landscape_init.toml"), "config-content").unwrap();
        std::fs::create_dir_all(staging.path().join("static")).unwrap();
        std::fs::write(staging.path().join("static/index.html"), "<html>").unwrap();

        build_package(staging.path(), &output).unwrap();

        // Verify the output file exists and is non-empty.
        assert!(output.exists());
        let meta = std::fs::metadata(&output).unwrap();
        assert!(meta.len() > 0);

        // Verify permissions on output.
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);

        // Read back and verify contents.
        let gz = std::fs::File::open(&output).unwrap();
        let decoder = GzDecoder::new(gz);
        let mut archive = tar::Archive::new(decoder);
        let mut found_files = Vec::new();
        for entry in archive.entries().unwrap() {
            let entry = entry.unwrap();
            let path = entry.header().path().unwrap().to_string_lossy().to_string();
            let mode = entry.header().mode().unwrap();
            found_files.push((path, mode));
        }

        assert!(found_files.contains(&("landscape-webserver".into(), 0o755)));
        assert!(found_files.contains(&("landscape_init.toml".into(), 0o644)));
        assert!(found_files.contains(&("static/index.html".into(), 0o644)));
    }

    #[test]
    fn sha256_file_computes_correctly() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.txt");
        std::fs::write(&path, "hello").unwrap();
        let hash = sha256_file(&path).unwrap();
        assert_eq!(hash, "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
    }

    #[test]
    fn timestamp_format() {
        let ts = timestamp();
        assert_eq!(ts.len(), 15); // "YYYYMMDD-HHMMSS"
        let parts: Vec<&str> = ts.split('-').collect();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].len(), 8);
        assert_eq!(parts[1].len(), 6);
    }

    #[test]
    fn rfc3339_format() {
        let ts = rfc3339();
        assert!(ts.contains('T'));
        assert!(ts.ends_with('Z'));
    }
}
