use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use super::plan::InstallError;

const MAX_PASSWORD_FILE_BYTES: u64 = 4 * 1024;

pub(crate) struct Credentials {
    pub admin_user: String,
    pub password: String,
}

pub(crate) fn validate_password(password: &str) -> Result<(), InstallError> {
    let bytes = password.as_bytes();
    if bytes.len() < 8 {
        return Err(InstallError::InvalidPassword(
            "password must be at least 8 bytes".into(),
        ));
    }
    if !bytes.iter().any(u8::is_ascii_lowercase) {
        return Err(InstallError::InvalidPassword(
            "password must contain an ASCII lowercase letter".into(),
        ));
    }
    if !bytes.iter().any(u8::is_ascii_uppercase) {
        return Err(InstallError::InvalidPassword(
            "password must contain an ASCII uppercase letter".into(),
        ));
    }
    if !bytes.iter().any(u8::is_ascii_digit) {
        return Err(InstallError::InvalidPassword(
            "password must contain an ASCII digit".into(),
        ));
    }
    Ok(())
}

pub(crate) fn read_password_file(path: &Path, required_uid: u32) -> Result<String, InstallError> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| {
            InstallError::InvalidPasswordFile(format!(
                "{} is not a readable regular file",
                path.display()
            ))
        })?;
    let metadata = file.metadata().map_err(InstallError::Io)?;
    if !metadata.is_file() {
        return Err(InstallError::InvalidPasswordFile(format!(
            "{} is not a regular file",
            path.display()
        )));
    }
    if metadata.uid() != required_uid {
        return Err(InstallError::InvalidPasswordFile(format!(
            "{} must be owned by uid {required_uid}",
            path.display()
        )));
    }
    if metadata.mode() & 0o077 != 0 {
        return Err(InstallError::InvalidPasswordFile(format!(
            "{} must not grant group or other permissions",
            path.display()
        )));
    }
    if metadata.len() > MAX_PASSWORD_FILE_BYTES {
        return Err(InstallError::InvalidPasswordFile(format!(
            "{} exceeds the 4 KiB limit",
            path.display()
        )));
    }
    let mut content = String::new();
    file.take(MAX_PASSWORD_FILE_BYTES + 1)
        .read_to_string(&mut content)
        .map_err(|_| {
            InstallError::InvalidPasswordFile(format!(
                "{} is not valid single-line UTF-8",
                path.display()
            ))
        })?;
    if content.contains('\0') {
        return Err(InstallError::InvalidPasswordFile(format!(
            "{} contains a NUL byte",
            path.display()
        )));
    }
    if content.ends_with("\r\n") {
        content.truncate(content.len() - 2);
    } else if content.ends_with('\n') {
        content.truncate(content.len() - 1);
    }
    if content.is_empty() {
        return Err(InstallError::InvalidPasswordFile(format!(
            "{} is empty",
            path.display()
        )));
    }
    if content.chars().any(char::is_control) {
        return Err(InstallError::InvalidPasswordFile(format!(
            "{} must be a single line without control characters",
            path.display()
        )));
    }
    Ok(content)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("lkit-cred-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn current_uid() -> u32 {
        unsafe { libc::geteuid() }
    }

    fn write_password_file(path: &Path, content: &[u8], mode: u32) {
        std::fs::write(path, content).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn validates_password_complexity() {
        assert!(validate_password("Secret123").is_ok());
        assert!(validate_password("Secret123秘密").is_ok());
        assert!(validate_password("Sec123").is_err());
        assert!(validate_password("secret123").is_err());
        assert!(validate_password("SECRET123").is_err());
        assert!(validate_password("Secretabc").is_err());
    }

    #[test]
    fn reads_valid_password_file() {
        let dir = temp_dir("valid");
        let path = dir.join("pw");
        write_password_file(&path, b"Secret123\n", 0o600);
        assert_eq!(
            read_password_file(&path, current_uid()).unwrap(),
            "Secret123"
        );
        write_password_file(&path, b"Secret123\r\n", 0o600);
        assert_eq!(
            read_password_file(&path, current_uid()).unwrap(),
            "Secret123"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_symlink_password_file() {
        let dir = temp_dir("symlink");
        let target = dir.join("target");
        write_password_file(&target, b"Secret123\n", 0o600);
        let link = dir.join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(read_password_file(&link, current_uid()).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_invalid_password_files() {
        let dir = temp_dir("invalid");
        let path = dir.join("pw");
        write_password_file(&path, b"Secret123\n", 0o600);

        write_password_file(&path, b"abc\ndef\n", 0o600);
        assert!(read_password_file(&path, current_uid()).is_err());

        write_password_file(&path, b"Secret\x00123\n", 0o600);
        assert!(read_password_file(&path, current_uid()).is_err());

        write_password_file(&path, b"\n", 0o600);
        assert!(read_password_file(&path, current_uid()).is_err());

        write_password_file(&path, &[0xFF; 8], 0o600);
        assert!(read_password_file(&path, current_uid()).is_err());

        let large = vec![b'a'; 4 * 1024 + 1];
        write_password_file(&path, &large, 0o600);
        assert!(read_password_file(&path, current_uid()).is_err());

        write_password_file(&path, b"Secret123\n", 0o644);
        assert!(read_password_file(&path, current_uid()).is_err());

        write_password_file(&path, b"Secret123\n", 0o600);
        assert!(read_password_file(&path, current_uid() + 1).is_err());

        assert!(read_password_file(&dir, current_uid()).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
