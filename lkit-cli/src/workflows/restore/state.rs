use std::path::Path;

use chrono::Utc;

use super::super::artifacts::{WEBSERVER_BINARY, hash_file};
use super::super::backup as lkb;
use super::super::backup::{BackupArchitecture, BackupMetadata};
use super::super::export;
use super::super::health::DocsProbe;
use super::super::pipeline;
use super::super::plan::InstallError;
use super::super::root::InstallRoot;
use super::super::state::{
    ArchiveAsset, Assets, InitStatus, InitializationState, InstallState, STATE_LAYOUT_VERSION,
    STATE_SCHEMA_VERSION, ServiceState, StateArchitecture, StateServiceManager, WebserverAsset,
};
use super::super::transaction::TransactionFile;
use super::RestoreOptions;
use crate::deployment::layout;

pub(super) async fn create_protection_backup<P: DocsProbe>(
    root: &InstallRoot,
    state: &InstallState,
    transaction: &mut TransactionFile,
    options: &RestoreOptions<'_, P>,
) -> Result<(), InstallError> {
    pipeline::check_initialization(root, state)?;
    pipeline::verify_current_backend(root, state)?;
    let token = (options.token)()?;
    let exported = export::export_config(&options.export_base_url, &token).await?;
    if exported.version != state.active_version {
        return Err(InstallError::ExportFailed(format!(
            "exported version {} does not match the running version {}",
            exported.version, state.active_version
        )));
    }
    let architecture = match state.assets.webserver.architecture {
        StateArchitecture::X86_64 => "x86_64",
        StateArchitecture::Aarch64 => "aarch64",
    };
    let version = pipeline::parse_stable_version(&state.active_version).map_err(|error| {
        InstallError::CorruptedState(format!("invalid active version: {error}"))
    })?;
    let webserver = root
        .canonical
        .join("releases")
        .join(&state.active_version)
        .join(WEBSERVER_BINARY);
    let static_dir = root.canonical.join("current/static");
    let static_archive = root
        .canonical
        .join("releases")
        .join(&state.active_version)
        .join("static.zip");
    let geo_tmp = root.canonical.join("data/geo_tmp");
    let backup_ref = lkb::create_backup(
        &layout::territory_backups_dir(),
        &version,
        architecture,
        &webserver,
        &exported.content,
        &static_dir,
        &static_archive,
        &geo_tmp,
        &crate::tr!(crate::keys::BACKUP_AUTO_REMARK_RESTORE),
        true,
        None,
    )?;
    transaction.backup = Some(backup_ref);
    Ok(())
}

/// 恢复提交的 state:`repository` 沿用当前安装,`webserver` 与 `static_archive`
/// 身份分别从解包二进制和备份内压缩包现场计算。
pub(super) fn build_restore_state(
    root: &InstallRoot,
    _previous: &InstallState,
    transaction: &TransactionFile,
    metadata: &BackupMetadata,
    restore_dir: &Path,
    unit_sha: Option<String>,
) -> Result<InstallState, InstallError> {
    let binary = restore_dir.join("landscape-webserver");
    let (webserver_sha256, webserver_size) = hash_file(&binary)?;
    let static_zip = restore_dir.join("static.zip");
    let (static_sha256, static_size) = hash_file(&static_zip)?;
    let architecture = match metadata.architecture {
        BackupArchitecture::X86_64 => StateArchitecture::X86_64,
        BackupArchitecture::Aarch64 => StateArchitecture::Aarch64,
    };
    let initialization = InitializationState {
        status: InitStatus::Complete,
        lock_present: true,
        initialized_at: Some(Utc::now()),
    };
    let service = ServiceState {
        manager: StateServiceManager::Systemd,
        registered: true,
        enabled: true,
        verified: true,
        definition_path: Some("service/landscape-router.service".into()),
        definition_sha256: unit_sha,
    };
    Ok(InstallState {
        schema_version: STATE_SCHEMA_VERSION,
        layout_version: STATE_LAYOUT_VERSION,
        install_root: root.install_root.display().to_string(),
        canonical_install_root: root.canonical.display().to_string(),
        active_version: metadata.landscape_version.clone(),
        assets: Assets {
            webserver: WebserverAsset {
                architecture,
                sha256: webserver_sha256,
                size: webserver_size,
            },
            static_archive: ArchiveAsset {
                sha256: static_sha256,
                size: static_size,
            },
        },
        initialization,
        service,
        last_transaction_id: Some(transaction.transaction_id.clone()),
        committed_at: Some(Utc::now()),
    })
}

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
mod tests {
    // 交互模式测试通过 std::sync::Mutex 串行化,锁故意跨 await 持有。
    use super::*;

    use super::super::tests::{
        NonInteractiveGuard, PAYLOAD_1_3_0, TOKEN, YES, ZIP_1_3_0, activate_version,
        create_target_backup, fake_systemd_stateful, init_watcher, install_state,
        interactive_guard, none_health, setup_current, temp_root, write_unit_origin,
    };
    use super::super::{RestoreArgs, RestoreOptions, RestoreOutcome, restore_version};
    use crate::deployment::state::load_state;
    use crate::deployment::state::write_state;
    use crate::deployment::transaction::find_unfinished;
    use crate::release::repository::test_server::{TestResponse, TestServer};

    #[tokio::test]
    async fn restore_blocks_without_allow_no_backup_when_protection_fails() {
        let _guard = interactive_guard().await;
        crate::interaction::interactive::configure(false);
        let root = temp_root("protection-blocked");
        let territory = root.join("territory");
        std::fs::create_dir_all(&territory).unwrap();
        let _territory_guard = crate::deployment::layout::test_territory(&territory);
        let install_root = InstallRoot {
            install_root: root.clone(),
            canonical: root.clone(),
        };
        activate_version(&install_root, "1.3.0", PAYLOAD_1_3_0, ZIP_1_3_0);
        setup_current(&install_root);
        let systemd = fake_systemd_stateful(&root.join("fake-systemd"));
        let state = install_state(&install_root, "1.3.0", PAYLOAD_1_3_0, ZIP_1_3_0);
        write_state(&install_root, &state).unwrap();
        let (backup_ref, _) = create_target_backup(&install_root);
        let server = TestServer::start(|_| TestResponse::status(500, "boom", Vec::new()));
        let options = RestoreOptions {
            export_base_url: server.base.clone(),
            token: &TOKEN,
            confirm: &YES,
            health: &none_health(),
        };
        let args = RestoreArgs {
            backup_id: Some(backup_ref.backup_id),
            file_path: None,
            allow_no_backup: false,
            yes: false,
            console_confirmed: false,
        };
        assert!(
            restore_version(&install_root, &state, &systemd, &args, &options)
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read_link(install_root.canonical.join("current")).unwrap(),
            std::path::PathBuf::from("releases/1.3.0")
        );
        assert_eq!(
            load_state(&install_root).unwrap().unwrap().active_version,
            "1.3.0"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn restore_continues_with_allow_no_backup_when_protection_fails() {
        // 保护备份失败时默认阻断;显式 --allow-no-backup 才允许继续,
        // 事务记录 no_backup: true 且不记录保护 .lkb。
        let _guard = interactive_guard().await;
        crate::interaction::interactive::configure(true);
        let _reset = NonInteractiveGuard;
        let root = temp_root("protection-allow");
        let territory = root.join("territory");
        std::fs::create_dir_all(&territory).unwrap();
        let _territory_guard = crate::deployment::layout::test_territory(&territory);
        let install_root = InstallRoot {
            install_root: root.clone(),
            canonical: root.clone(),
        };
        activate_version(&install_root, "1.3.0", PAYLOAD_1_3_0, ZIP_1_3_0);
        setup_current(&install_root);
        write_unit_origin(&install_root);
        let systemd = fake_systemd_stateful(&root.join("fake-systemd"));
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watcher = init_watcher(install_root.canonical.join("data"), stop.clone());
        let state = install_state(&install_root, "1.3.0", PAYLOAD_1_3_0, ZIP_1_3_0);
        write_state(&install_root, &state).unwrap();
        let (backup_ref, _) = create_target_backup(&install_root);
        let server = TestServer::start(|_| TestResponse::status(500, "boom", Vec::new()));
        let options = RestoreOptions {
            export_base_url: server.base.clone(),
            token: &TOKEN,
            confirm: &YES,
            health: &none_health(),
        };
        let args = RestoreArgs {
            backup_id: Some(backup_ref.backup_id),
            file_path: None,
            allow_no_backup: true,
            yes: true,
            console_confirmed: false,
        };
        assert!(matches!(
            restore_version(&install_root, &state, &systemd, &args, &options).await,
            Ok(RestoreOutcome::Committed { .. })
        ));
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        watcher.join().unwrap();
        let transaction = find_unfinished(&install_root).unwrap();
        assert!(
            transaction.is_none(),
            "the restore must commit successfully"
        );
        let tx_id = load_state(&install_root)
            .unwrap()
            .unwrap()
            .last_transaction_id
            .unwrap();
        let path =
            crate::deployment::layout::territory_transactions_dir().join(format!("{tx_id}.json"));
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["no_backup"], true);
        assert!(value["backup"].is_null());
        assert!(value["restore_backup"].is_object());
        let _ = std::fs::remove_dir_all(&root);
    }
}
