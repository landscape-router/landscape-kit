//! Reject downgrades that cannot safely read the retained transaction history.
use crate::deployment::layout;
use crate::deployment::plan::InstallError;
use crate::deployment::transaction::{TransactionFile, validate_transaction};
use std::path::Path;

pub(super) fn ensure_downgrade_compatible() -> Result<(), InstallError> {
    inspect_transactions(&layout::territory_transactions_dir())
}

fn unverifiable(path: &Path, reason: impl std::fmt::Display) -> InstallError {
    InstallError::Preflight(format!(
        "cannot verify downgrade compatibility of {}: {reason}; keep the current lkit version until the transaction history can be verified",
        path.display()
    ))
}

fn inspect_transactions(directory: &Path) -> Result<(), InstallError> {
    match std::fs::symlink_metadata(directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(unverifiable(directory, error)),
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => {
            return Err(unverifiable(
                directory,
                "transaction directory is not a real directory",
            ));
        }
    }
    let entries = std::fs::read_dir(directory).map_err(|error| unverifiable(directory, error))?;
    for entry in entries {
        let path = entry
            .map_err(|error| unverifiable(directory, error))?
            .path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let metadata =
            std::fs::symlink_metadata(&path).map_err(|error| unverifiable(&path, error))?;
        if !metadata.file_type().is_file() {
            return Err(unverifiable(
                &path,
                "transaction is not a regular JSON file",
            ));
        }
        let bytes = std::fs::read(&path).map_err(|error| unverifiable(&path, error))?;
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|error| unverifiable(&path, error))?;
        if value
            .get("systemd_before")
            .and_then(|before| before.get("enabled"))
            .is_some_and(serde_json::Value::is_object)
        {
            return Err(InstallError::Preflight(format!(
                "cannot downgrade lkit: {} contains an exact systemd enablement snapshot, including retained terminal history; keep this version or use a release that supports service enablement snapshot version 1",
                path.display()
            )));
        }
        // Parsing only the enabled field would silently accept damaged or
        // unknown records that an older daemon also cannot safely interpret.
        let transaction: TransactionFile =
            serde_json::from_value(value).map_err(|error| unverifiable(&path, error))?;
        validate_transaction(&transaction).map_err(|error| unverifiable(&path, error))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deployment::root::InstallRoot;
    use serde_json::{Value, json};
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("lkit-downgrade-{}", uuid::Uuid::now_v7()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn transaction(&self) -> Value {
            let root = InstallRoot {
                install_root: self.0.join("installation"),
                canonical: self.0.join("installation"),
            };
            serde_json::to_value(
                TransactionFile::new_install(&root, &semver::Version::new(1, 2, 3)).unwrap(),
            )
            .unwrap()
        }
        fn write(&self, value: &Value) {
            std::fs::write(
                self.0.join("transaction.json"),
                serde_json::to_vec(value).unwrap(),
            )
            .unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn absent_history_and_non_transaction_workspaces_allow_downgrade() {
        let fixture = Fixture::new();
        assert!(inspect_transactions(&fixture.0.join("missing")).is_ok());
        std::fs::write(fixture.0.join("interrupted.tmp"), "not json").unwrap();
        std::fs::create_dir(fixture.0.join("workspace")).unwrap();
        std::fs::write(
            fixture.0.join("workspace/snapshot.json"),
            "not a transaction",
        )
        .unwrap();
        assert!(inspect_transactions(&fixture.0).is_ok());
    }

    #[test]
    fn legacy_boolean_and_absent_service_snapshots_remain_compatible() {
        let fixture = Fixture::new();
        for phase in ["preparing", "committed", "rolled_back", "failed"] {
            for before in [
                Value::Null,
                json!({"registration": {"kind": "missing", "target": null}, "enabled": false, "active": false}),
                json!({"registration": {"kind": "symlink", "target": fixture.0.join("installation/service/landscape-router.service")}, "enabled": true, "active": true}),
            ] {
                let mut value = fixture.transaction();
                value["phase"] = phase.into();
                value["systemd_before"] = before;
                fixture.write(&value);
                assert!(
                    inspect_transactions(&fixture.0).is_ok(),
                    "legacy phase {phase}"
                );
            }
        }
    }

    #[test]
    fn exact_snapshot_blocks_downgrade_even_for_terminal_or_foreign_history() {
        let fixture = Fixture::new();
        for phase in ["preparing", "committed", "rolled_back", "failed"] {
            let mut value = fixture.transaction();
            value["phase"] = phase.into();
            value["canonical_install_root"] = fixture
                .0
                .join("previous-installation")
                .display()
                .to_string()
                .into();
            value["systemd_before"] = json!({"enabled": {"version": 1, "enabled": false}});
            fixture.write(&value);
            let error = inspect_transactions(&fixture.0).unwrap_err().to_string();
            assert!(
                error.contains("service enablement snapshot version 1"),
                "{error}"
            );
            assert_eq!(
                serde_json::from_slice::<Value>(
                    &std::fs::read(fixture.0.join("transaction.json")).unwrap()
                )
                .unwrap(),
                value,
                "history must remain untouched"
            );
        }
    }

    #[test]
    fn malformed_or_unknown_history_fails_closed() {
        let fixture = Fixture::new();
        for bytes in [
            "{",
            "null",
            "{}",
            "{\"systemd_before\":{\"enabled\":\"true\"}}",
        ] {
            std::fs::write(fixture.0.join("transaction.json"), bytes).unwrap();
            assert!(inspect_transactions(&fixture.0).is_err());
        }
        let mut value = fixture.transaction();
        value["schema_version"] = 999.into();
        fixture.write(&value);
        assert!(inspect_transactions(&fixture.0).is_err());
    }

    #[test]
    fn non_regular_json_entries_fail_closed_without_following_them() {
        for missing in [false, true] {
            let fixture = Fixture::new();
            let target = fixture.0.join("outside");
            if !missing {
                std::fs::write(&target, serde_json::to_vec(&fixture.transaction()).unwrap())
                    .unwrap();
            }
            symlink(&target, fixture.0.join("transaction.json")).unwrap();
            assert!(inspect_transactions(&fixture.0).is_err());
        }
        let fixture = Fixture::new();
        std::fs::create_dir(fixture.0.join("transaction.json")).unwrap();
        assert!(inspect_transactions(&fixture.0).is_err());
    }

    #[test]
    fn redirected_transaction_directory_fails_closed() {
        let fixture = Fixture::new();
        let target = fixture.0.join("actual");
        std::fs::create_dir(&target).unwrap();
        let redirected = fixture.0.join("redirected");
        symlink(target, &redirected).unwrap();
        assert!(inspect_transactions(&redirected).is_err());
    }
}
