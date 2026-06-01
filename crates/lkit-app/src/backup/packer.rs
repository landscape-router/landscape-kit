//! .lkb file format: streaming build, parse, and checksum verification.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha256};
use tar::{Archive, Builder};

use lkit_core::{BackupMetadata, HEADER_SIZE, HEADER_VERSION, MAGIC, META_REGION_SIZE};

use crate::AppError;

/// Build a tar.gz archive from a staging directory, streaming to `writer` starting at offset.
///
/// Returns the SHA256 hex digest of the tar.gz data written.
pub fn build_archive_to(
    staging_dir: &Path,
    writer: &mut (impl Write + Seek),
    offset: u64,
) -> Result<String, AppError> {
    writer
        .seek(SeekFrom::Start(offset))
        .map_err(|e| AppError::Backup(format!("seek failed: {e}")))?;

    let encoder = GzEncoder::new(&mut *writer, Compression::default());
    let mut hasher = Sha256::new();
    let mut tee = TeeWriter::new(encoder, &mut hasher);
    {
        let mut tar = Builder::new(&mut tee);
        tar.append_dir_all(".", staging_dir)
            .map_err(|e| AppError::Backup(format!("tar append failed: {e}")))?;
        tar.finish().map_err(|e| AppError::Backup(format!("tar finish failed: {e}")))?;
    }
    tee.flush().map_err(|e| AppError::Backup(format!("flush failed: {e}")))?;

    let digest = hasher.finalize();
    Ok(format!("sha256:{digest:x}"))
}

/// Write the 1 MiB metadata region (header + JSON + zero-padding).
pub fn write_meta_region(
    writer: &mut (impl Write + Seek),
    metadata: &BackupMetadata,
) -> Result<(), AppError> {
    writer
        .seek(SeekFrom::Start(0))
        .map_err(|e| AppError::Backup(format!("seek to 0 failed: {e}")))?;

    let json_bytes = serde_json::to_vec(metadata)
        .map_err(|e| AppError::Backup(format!("serialize metadata: {e}")))?;
    let json_len = json_bytes.len();
    if json_len as u32 > lkit_core::MAX_JSON_LEN {
        return Err(AppError::Backup(format!(
            "metadata JSON too large: {json_len} bytes (max {})",
            lkit_core::MAX_JSON_LEN
        )));
    }

    // 32-byte header
    writer.write_all(MAGIC).map_err(|e| AppError::Backup(format!("write magic: {e}")))?;
    writer
        .write_all(&HEADER_VERSION.to_le_bytes())
        .map_err(|e| AppError::Backup(format!("write version: {e}")))?;
    writer
        .write_all(&(json_len as u32).to_le_bytes())
        .map_err(|e| AppError::Backup(format!("write json_len: {e}")))?;
    writer.write_all(&[0u8; 6]).map_err(|e| AppError::Backup(format!("write reserved1: {e}")))?;
    writer.write_all(&[0u8; 16]).map_err(|e| AppError::Backup(format!("write reserved2: {e}")))?;

    // JSON body
    writer.write_all(&json_bytes).map_err(|e| AppError::Backup(format!("write json: {e}")))?;

    // Zero-pad to 1 MiB
    let padding = META_REGION_SIZE as usize - HEADER_SIZE - json_len;
    let zeros = vec![0u8; padding];
    writer.write_all(&zeros).map_err(|e| AppError::Backup(format!("write padding: {e}")))?;

    Ok(())
}

/// Parse the metadata from a .lkb file without touching the tar.gz data.
pub fn read_metadata(reader: &mut (impl Read + Seek)) -> Result<BackupMetadata, AppError> {
    reader.seek(SeekFrom::Start(0)).map_err(|e| AppError::Backup(format!("seek to 0: {e}")))?;

    let mut header = [0u8; HEADER_SIZE];
    reader.read_exact(&mut header).map_err(|e| AppError::Backup(format!("read header: {e}")))?;

    // Validate magic
    if header[0..4] != *MAGIC {
        return Err(AppError::BackupCorrupted(format!(
            "magic mismatch: expected {:?}, got {:?}",
            std::str::from_utf8(MAGIC).unwrap_or("??"),
            std::str::from_utf8(&header[0..4]).unwrap_or("??")
        )));
    }

    // Validate version
    let version = u16::from_le_bytes([header[4], header[5]]);
    if version != HEADER_VERSION {
        return Err(AppError::BackupCorrupted(format!("unsupported version: {version}")));
    }

    // Read json_len
    let json_len = u32::from_le_bytes([header[6], header[7], header[8], header[9]]) as usize;
    if json_len == 0 || json_len > lkit_core::MAX_JSON_LEN as usize {
        return Err(AppError::BackupCorrupted(format!("invalid json_len: {json_len}")));
    }

    // Read JSON
    let mut json_bytes = vec![0u8; json_len];
    reader
        .read_exact(&mut json_bytes)
        .map_err(|e| AppError::Backup(format!("read json body: {e}")))?;

    let metadata: BackupMetadata = serde_json::from_slice(&json_bytes)
        .map_err(|e| AppError::BackupCorrupted(format!("metadata parse: {e}")))?;

    Ok(metadata)
}

/// Stream-verify checksum while extracting tar.gz to `target_dir`.
pub fn extract_verified(
    reader: &mut (impl Read + Seek),
    expected_checksum: &str,
    target_dir: &Path,
) -> Result<(), AppError> {
    reader
        .seek(SeekFrom::Start(META_REGION_SIZE))
        .map_err(|e| AppError::Backup(format!("seek to tar.gz: {e}")))?;

    let mut hasher = Sha256::new();
    let mut tee = TeeReader::new(&mut *reader, &mut hasher);
    let decoder = GzDecoder::new(&mut tee);
    let mut archive = Archive::new(decoder);

    archive.unpack(target_dir).map_err(|e| AppError::Backup(format!("extract failed: {e}")))?;

    let digest = hasher.finalize();
    let actual = format!("sha256:{digest:x}");
    if actual != expected_checksum {
        return Err(AppError::ChecksumMismatch);
    }

    Ok(())
}

/// Compute the SHA256 of the tar.gz section in a .lkb file.
pub fn compute_targz_checksum(reader: &mut (impl Read + Seek)) -> Result<String, AppError> {
    reader
        .seek(SeekFrom::Start(META_REGION_SIZE))
        .map_err(|e| AppError::Backup(format!("seek: {e}")))?;

    let mut hasher = Sha256::new();
    std::io::copy(reader, &mut hasher)
        .map_err(|e| AppError::Backup(format!("read tar.gz: {e}")))?;

    let digest = hasher.finalize();
    Ok(format!("sha256:{digest:x}"))
}

/// A writer that tees output to a Sha256 hasher.
struct TeeWriter<'a, W: Write> {
    inner: W,
    hasher: &'a mut Sha256,
}

impl<'a, W: Write> TeeWriter<'a, W> {
    fn new(inner: W, hasher: &'a mut Sha256) -> Self {
        Self { inner, hasher }
    }
}

impl<'a, W: Write> Write for TeeWriter<'a, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.hasher.update(buf);
        self.inner.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// A reader that tees input to a Sha256 hasher.
struct TeeReader<'a, R: Read> {
    inner: R,
    hasher: &'a mut Sha256,
}

impl<'a, R: Read> TeeReader<'a, R> {
    fn new(inner: R, hasher: &'a mut Sha256) -> Self {
        Self { inner, hasher }
    }
}

impl<'a, R: Read> Read for TeeReader<'a, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hasher.update(&buf[..n]);
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lkit_core::BackupScope;
    use std::io::Cursor;

    #[test]
    fn test_read_metadata_magic_mismatch() {
        let buf = vec![0u8; 1024 * 1024];
        let mut cursor = Cursor::new(buf);
        let result = read_metadata(&mut cursor);
        match result {
            Err(AppError::BackupCorrupted(msg)) => assert!(msg.contains("magic")),
            _ => panic!("expected BackupCorrupted"),
        }
    }

    #[test]
    fn test_write_and_read_metadata_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
        let metadata = BackupMetadata {
            backup_id: "test-id".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            landscape_version: "1.0.0".into(),
            lkit_version: "0.1.0".into(),
            hostname: "test".into(),
            remark: None,
            auto: false,
            scope: BackupScope::Minimal,
            checksum: "sha256:abcd1234".into(),
        };

        let mut buf = vec![0u8; META_REGION_SIZE as usize + 100];
        let mut cursor = Cursor::new(&mut buf[..]);

        write_meta_region(&mut cursor, &metadata)?;

        cursor.seek(SeekFrom::Start(0))?;
        let decoded = read_metadata(&mut cursor)?;

        assert_eq!(decoded.backup_id, "test-id");
        assert_eq!(decoded.checksum, "sha256:abcd1234");
        Ok(())
    }
}
