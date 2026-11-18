//! Local password verification for a Flare mapping session. No login shell or SSH service.
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::deployment::{layout, state};
use landscape_terrain_proto::protocol::v6;
use serde::Deserialize;
use subtle::ConstantTimeEq;
use yescrypt::PasswordVerifier;
use zeroize::Zeroizing;

fn protected_file(
    path: &Path,
    max: u64,
    owner: u32,
    private: bool,
) -> io::Result<Zeroizing<Vec<u8>>> {
    // A trusted final inode alone is insufficient when a parent can be replaced.
    for parent in path.ancestors().skip(1) {
        let directory = fs::symlink_metadata(parent)?;
        let trusted_owner = directory.uid() == 0 || directory.uid() == owner;
        let protected_entries = directory.mode() & 0o022 == 0 || directory.mode() & 0o1000 != 0;
        if !directory.is_dir() || !trusted_owner || !protected_entries {
            return Err(io::Error::other("untrusted credential directory"));
        }
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.uid() != owner
        || meta.mode() & (if private { 0o077 } else { 0o022 }) != 0
    {
        return Err(io::Error::other("untrusted credential file"));
    }
    let mut bytes = Zeroizing::new(Vec::new());
    (&mut file).take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(io::Error::other("credential file too large"));
    }
    Ok(bytes)
}

fn read_identity(path: &Path) -> io::Result<Zeroizing<[u8; 32]>> {
    let raw = protected_file(path, 32, 0, true)?;
    Ok(Zeroizing::new(
        raw.as_slice()
            .try_into()
            .map_err(|_| io::Error::other("invalid Flare identity"))?,
    ))
}

pub(crate) fn fingerprint() -> io::Result<String> {
    let private = read_identity(&layout::lkit_territory().join(layout::FLARE_IDENTITY_FILE))?;
    Ok(v6::fingerprint(&v6::public_key(&private)))
}

pub(super) fn identity() -> io::Result<Zeroizing<[u8; 32]>> {
    let dir = layout::lkit_territory();
    fs::create_dir_all(dir)?;
    let path = dir.join(layout::FLARE_IDENTITY_FILE);
    match read_identity(&path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let staging = layout::territory_run_dir();
            fs::create_dir_all(&staging)?;
            let meta = fs::symlink_metadata(&staging)?;
            if !meta.is_dir() || meta.uid() != 0 || meta.mode() & 0o022 != 0 {
                return Err(io::Error::other(
                    "untrusted Flare identity staging directory",
                ));
            }
            let tmp = staging.join(format!(".flare-{}.tmp", uuid::Uuid::now_v7()));
            let mut out = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&tmp)?;
            let key = Zeroizing::new(rand::random::<[u8; 32]>());
            let result = (|| {
                out.write_all(&*key)?;
                out.sync_all()?;
                match fs::hard_link(&tmp, &path) {
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
                    r => r,
                }
            })();
            fs::remove_file(tmp)?;
            result?;
            fs::File::open(dir)?.sync_all()?;
        }
        Err(e) => return Err(e),
        Ok(key) => return Ok(key),
    }
    read_identity(&path)
}

#[derive(Deserialize)]
struct Config {
    auth: Credentials,
}
#[derive(Deserialize)]
struct Credentials {
    admin_user: String,
    admin_pass: String,
}

fn landscape(user: &str, password: &[u8]) -> Option<bool> {
    let root = state::discover_landscape_root().ok()??;
    landscape_file(
        &root.canonical.join("data/landscape.toml"),
        0,
        user,
        password,
    )
}

fn landscape_file(path: &Path, owner: u32, user: &str, password: &[u8]) -> Option<bool> {
    let config = protected_file(path, 4 * 1024 * 1024, owner, false).ok()?;
    let config: Config = toml::from_str(std::str::from_utf8(&config).ok()?).ok()?;
    let pass = Zeroizing::new(config.auth.admin_pass);
    Some(bool::from(
        user.as_bytes().ct_eq(config.auth.admin_user.as_bytes()) & password.ct_eq(pass.as_bytes()),
    ))
}

fn verify_shadow(text: &str, password: &[u8], today: i64) -> Option<bool> {
    let line = text.lines().find(|line| line.starts_with("root:"))?;
    let fields: Vec<_> = line.split(':').collect();
    if fields.len() != 9 || password.is_empty() {
        return None;
    }
    let day = |i: usize| -> Option<i64> {
        match fields[i] {
            "" => Some(-1),
            value => value.parse().ok(),
        }
    };
    let last = day(2)?;
    let maximum = day(4)?;
    let expiry = day(7)?;
    if last == 0
        || (expiry >= 0 && today >= expiry)
        || (last > 0 && maximum >= 0 && today > last.checked_add(maximum)?)
    {
        return Some(false);
    }
    let hash = fields[1];
    Some(if hash.starts_with("$y$") {
        yescrypt::Yescrypt::default()
            .verify_password(password, hash)
            .is_ok()
    } else if hash.starts_with("$5$") || hash.starts_with("$6$") {
        sha_crypt::ShaCrypt::default()
            .verify_password(password, hash)
            .is_ok()
    } else {
        false
    })
}

/// The method byte is followed by a NUL-terminated username and the password.
pub(super) fn verify(credentials: Zeroizing<Vec<u8>>) -> bool {
    let Some((&method, body)) = credentials.split_first() else {
        return false;
    };
    let Some(split) = body.iter().position(|b| *b == 0) else {
        return false;
    };
    let Ok(user) = std::str::from_utf8(&body[..split]) else {
        return false;
    };
    let password = &body[split + 1..];
    if user.is_empty() || password.is_empty() || password.contains(&0) {
        return false;
    }
    match method {
        v6::AUTH_LANDSCAPE => landscape(user, password).unwrap_or(false),
        v6::AUTH_ROOT if user == "root" => {
            let Ok(data) = protected_file(Path::new("/etc/shadow"), 4 * 1024 * 1024, 0, false)
            else {
                return false;
            };
            let Ok(text) = std::str::from_utf8(&data) else {
                return false;
            };
            let Ok(time) = SystemTime::now().duration_since(UNIX_EPOCH) else {
                return false;
            };
            verify_shadow(text, password, (time.as_secs() / 86400) as i64).unwrap_or(false)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn late_landscape_config_and_rotation_are_read_on_each_login() {
        let dir = std::env::temp_dir().join(format!("flare-auth-{}", uuid::Uuid::now_v7()));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("landscape.toml");
        let uid = unsafe { libc::geteuid() };
        let accepts = |password| landscape_file(&path, uid, "operator", password) == Some(true);
        assert!(!accepts(b"first"));
        let write = |password| {
            fs::write(
                &path,
                format!("[auth]\nadmin_user = 'operator'\nadmin_pass = '{password}'"),
            )
            .unwrap()
        };
        write("first");
        assert!(accepts(b"first"));
        write("second");
        assert!(!accepts(b"first"));
        assert!(accepts(b"second"));
        fs::write(&path, "[auth]").unwrap();
        assert!(!accepts(b"second"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn verifies_sha512_crypt_root_without_external_authentication() {
        let hash = "$6$bbe605c2cce4c642$BiBOywFAm9kdv6ZPpj2GaKVqeh/.c21pf1uFBaq.e59KEE2Ej74iJleXaLXURYV6uh5LF4K7dDc4vtRtPiiKB/";
        let record = format!("root:{hash}:19000:0:99999:7:::");
        assert_eq!(verify_shadow(&record, b"foobar", 20000), Some(true));
        assert_eq!(verify_shadow(&record, b"wrong", 20000), Some(false));
    }

    #[test]
    fn root_password_respects_local_shadow_status() {
        // Published yescrypt reference vector for "pleaseletmein".
        let hash = "$y$j0/$LdJM$k7BXzSDuoGHW56SY3HxROCiA0gWRscZe2aA0q5oHPM0";
        let verify = |fields: &str, pass: &[u8]| verify_shadow(fields, pass, 20000) == Some(true);
        let valid = format!("root:{hash}:19000:0:99999:7:::");
        assert!(verify(&valid, b"pleaseletmein"));
        assert!(!verify(&valid, b"wrong"));
        for record in [
            valid.replace("root:", "admin:"),
            valid.replace(hash, ""),
            valid.replace(hash, "!"),
            valid.replace("99999", "100"),
            valid.replace("7:::", "7::19001:"),
            valid.replace("19000", "0"),
        ] {
            assert!(!verify(&record, b"pleaseletmein"));
        }
    }
}
