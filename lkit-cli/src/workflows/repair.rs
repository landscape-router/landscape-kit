use std::path::Path;

use chrono::Utc;

use super::artifacts::{WEBSERVER_BINARY, hash_str};
use super::backup;
use super::export;
use super::health::{DocsProbe, StartupOptions};
use super::manager::{ManagedService, ServiceManager};
use super::pipeline::{self, SwitchOptions};
use super::plan::InstallError;
use super::repository::{Architecture, Release, ReleaseProvider};
use super::rollback;
use super::root::InstallRoot;
use super::state::{
    ArchiveAsset, InitStatus, InstallState, StateArchitecture, StateServiceManager,
};
use super::transaction::{Phase, StaticBackupRef, TransactionFile};
use crate::deployment::layout;

#[derive(Debug)]
pub(crate) enum RepairOutcome {
    Committed,
    RolledBack,
    RollbackFailed { reason: String },
}

/// 校验新来源的落盘后端与状态记录完全一致(解压后比对)。
fn verify_backend_identity(
    built: &super::artifacts::BuiltRelease,
    state: &InstallState,
) -> Result<(), InstallError> {
    if built.webserver_sha256 != state.assets.webserver.sha256
        || built.webserver_size != state.assets.webserver.size
    {
        return Err(InstallError::UserRefused(
            "the new repository provides a different backend binary than the recorded installation; refusing to switch source"
                .into(),
        ));
    }
    Ok(())
}

/// 纯静态页面修复:不停止 Landscape、不创建 `.lkb`,也不做任何运行态检查。
/// 静态文件由运行中的 Landscape 热加载,因此只需备份现有 `static/`、
/// 原子替换为目标静态内容并提交状态;失败时从备份恢复。
///
/// 意图驱动:配置了激活的自定义前端源且未指定 `--official` 时,重新拉取该前端源
/// 的 latest/stable 并应用;否则恢复官方页面。官方路径成功后刷新版本目录
/// `static.zip` 并更新 state 中的 static archive 身份(可修复恢复/手工替换造成的
/// 身份漂移)。下载物仍与其解析来源的元数据严格校验,不再以 state 身份为门槛,
/// 因此 `repair static` 在任何状态下都可用。
///
/// 前端源解析是**宽容**的:config.toml 损坏、缺失或 active 指向不存在的 id 时一律
/// 按官方修复处理,绝不阻断。repair 是恢复工具,显式 `--repository` 在配置损坏时
/// 必须仍能绕过配置工作(e2e 契约),且修复官方页面永远安全。
pub(crate) async fn repair_static(
    root: &InstallRoot,
    provider: &ReleaseProvider,
    state: &InstallState,
    official: bool,
) -> Result<(), InstallError> {
    let architecture = architecture_from_state(state);
    let active = parse_active_version(state)?;
    let custom_source = if official {
        None
    } else {
        crate::deployment::config::resolve_active_frontend_lenient()
    };
    let release = provider.release(&active, architecture).await?;

    let mut transaction = TransactionFile::new_repair_static(root)?;
    super::transaction::begin(root, &transaction)?;
    let live_static = root.canonical.join("current/static");
    let mut activated = false;

    let result: Result<(), InstallError> = async {
        let tx_dir = tx_dir(root, &transaction);
        std::fs::create_dir_all(&tx_dir).map_err(InstallError::Io)?;
        let new_static = tx_dir.join("static");
        let backup_dir = tx_dir.join("static-backup");
        // 自定义前端源解析失败时:交互环境询问回退官方,非交互环境报错并提示
        // `--official`。前端源本身已宽容解析(配置损坏按官方处理),此处只处理
        // 源不可达/元数据非法。
        let mut official_restored = custom_source.is_none();
        if let Some(source) = custom_source.as_ref() {
            match crate::frontend::fetch_from_source(source, &active, &tx_dir).await {
                Ok(()) => {}
                Err(error) => {
                    let fallback = if crate::interaction::interactive::is_non_interactive() {
                        false
                    } else {
                        crate::interaction::interactive::confirm(&format!(
                            "{error}\nfall back to the official frontend pages?"
                        ))?
                    };
                    if !fallback {
                        return Err(InstallError::FrontendSource(format!(
                            "{error}; use `lkit repair static --official` to restore the official frontend pages"
                        )));
                    }
                    eprintln!(
                        "repair: {}",
                        crate::tr!(crate::keys::REPAIR_STATIC_FALLBACK_OFFICIAL)
                    );
                    pipeline::fetch_static_asset(&release, &tx_dir).await?;
                    official_restored = true;
                }
            }
        } else {
            pipeline::fetch_static_asset(&release, &tx_dir).await?;
        }
        if official_restored {
            refresh_version_static_zip(root, &state.active_version, &tx_dir.join("static.zip"))?;
        }
        rollback::copy_tree_into(&live_static, &backup_dir)?;
        transaction.static_backup = Some(StaticBackupRef {
            path: format!("transactions/{}/static-backup", transaction.transaction_id),
            target: format!("releases/{}/static", state.active_version),
        });
        super::transaction::mark_phase(root, &transaction, Phase::Prepared)?;

        super::transaction::mark_phase(root, &transaction, Phase::Activating)?;
        activated = true;
        let replaced = tx_dir.join("static-replaced");
        std::fs::rename(&live_static, &replaced).map_err(InstallError::Io)?;
        std::fs::rename(&new_static, &live_static).map_err(|error| {
            let _ = std::fs::rename(&replaced, &live_static);
            InstallError::Io(error)
        })?;
        let mut updated = state.clone();
        if official_restored {
            updated.assets.static_archive = ArchiveAsset {
                sha256: release.assets.static_archive.sha256.clone(),
                size: release.assets.static_archive.size,
            };
        }
        updated.last_transaction_id = Some(transaction.transaction_id.clone());
        updated.committed_at = Some(Utc::now());
        super::state::write_state(root, &updated)?;
        super::transaction::mark_phase(root, &transaction, Phase::Committed)?;
        if official_restored && custom_source.is_some() {
            eprintln!(
                "repair: {}",
                crate::tr!(crate::keys::REPAIR_STATIC_OFFICIAL_REAPPLY_NOTE)
            );
        }
        Ok(())
    }
    .await;

    match result {
        Ok(()) => Ok(()),
        Err(error) if activated => {
            if let Err(restore_error) = restore_static(&live_static, &tx_dir(root, &transaction)) {
                let _ = super::transaction::mark_phase(root, &transaction, Phase::Failed);
                return Err(InstallError::Io(std::io::Error::other(format!(
                    "{error}; additionally restoring the previous static pages failed: {restore_error}"
                ))));
            }
            let _ = super::transaction::mark_phase(root, &transaction, Phase::RolledBack);
            Err(error)
        }
        Err(error) => {
            let _ = super::transaction::mark_phase(root, &transaction, Phase::Failed);
            Err(error)
        }
    }
}

/// 官方路径成功后,把本次下载校验过的官方 `static.zip` 刷新到版本目录,修复
/// 恢复/手工替换造成的身份漂移。经同目录临时文件 + rename 原子完成。
fn refresh_version_static_zip(
    root: &InstallRoot,
    active_version: &str,
    downloaded: &Path,
) -> Result<(), InstallError> {
    let target = root
        .canonical
        .join("releases")
        .join(active_version)
        .join("static.zip");
    let tmp = target.with_extension("zip.tmp");
    std::fs::copy(downloaded, &tmp).map_err(InstallError::Io)?;
    std::fs::rename(&tmp, &target).map_err(InstallError::Io)?;
    Ok(())
}

fn restore_static(live_static: &Path, tx_dir: &Path) -> Result<(), InstallError> {
    let _ = std::fs::remove_dir_all(live_static);
    rollback::copy_tree_into(&tx_dir.join("static-backup"), live_static)
}

/// 后端 repair:从状态记录的可信仓库重新获取完全相同的资产,
/// 在停止服务前通过导出 API 创建并完整校验 `.lkb`,替换并验证同版本后端。
/// 修复前二进制另存到事务诊断目录;systemd 环境启动失败时按配置级流程回滚。
pub(crate) async fn repair_binary<P: DocsProbe>(
    root: &InstallRoot,
    provider: &ReleaseProvider,
    state: &InstallState,
    manager: &dyn ServiceManager,
    options: &SwitchOptions<'_, P>,
) -> Result<RepairOutcome, InstallError> {
    let architecture = architecture_from_state(state);
    let active = parse_active_version(state)?;
    let release = provider.release(&active, architecture).await?;
    pipeline::check_initialization(root, state)?;
    let is_systemd = state.service.manager == StateServiceManager::Systemd;

    let mut transaction = TransactionFile::new_repair_binary(root, &active)?;
    super::transaction::begin(root, &transaction)?;
    let mut activated = false;

    let result: Result<(), InstallError> = async {
        let tx_dir = tx_dir(root, &transaction);
        std::fs::create_dir_all(&tx_dir).map_err(InstallError::Io)?;
        let new_binary_dir = tx_dir.join("new-binary");
        std::fs::create_dir_all(&new_binary_dir).map_err(InstallError::Io)?;
        let built = pipeline::fetch_webserver_asset(&release, &new_binary_dir).await?;
        verify_backend_identity(&built, state)?;
        let current_binary = root
            .canonical
            .join("releases")
            .join(&state.active_version)
            .join(WEBSERVER_BINARY);
        std::fs::copy(&current_binary, tx_dir.join("repaired-binary")).map_err(InstallError::Io)?;

        let token = (options.token)()?;
        let exported = export::export_config(&options.export_base_url, &token).await?;
        if exported.version != state.active_version {
            return Err(InstallError::ExportFailed(format!(
                "exported version {} does not match the running version {}",
                exported.version, state.active_version
            )));
        }
        let static_dir = root.canonical.join("current/static");
        let geo_tmp = root.canonical.join("data/geo_tmp");
        let backup_ref = backup::create_backup(
            &layout::territory_backups_dir(),
            &active,
            architecture.key(),
            &current_binary,
            &exported.content,
            &static_dir,
            &geo_tmp,
            &crate::tr!(crate::keys::BACKUP_AUTO_REMARK_REPAIR),
            true,
            None,
        )?;
        transaction.backup = Some(backup_ref);

        let unit_sha = if is_systemd {
            transaction.systemd_before = Some(super::pipeline::capture_before(
                manager,
                ManagedService::LandscapeRouter,
            )?);
            let backup_dir = layout::territory_backups_dir()
                .join(&transaction.transaction_id)
                .join("host/resolv.conf");
            let _ = super::resolv::backup(manager.resolv_conf(), &backup_dir)?;
            transaction.resolv_conf_backup = Some(format!(
                "backups/{}/host/resolv.conf",
                transaction.transaction_id
            ));
            let origin = root.canonical.join("service/landscape-router.service");
            Some(hash_str(
                &std::fs::read_to_string(origin).map_err(InstallError::Io)?,
            ))
        } else {
            None
        };
        rollback::write_state_snapshot(root, &transaction.transaction_id, state)?;
        super::transaction::mark_phase(root, &transaction, Phase::Prepared)?;

        if is_systemd {
            super::transaction::mark_phase(root, &transaction, Phase::Stopping)?;
            manager.stop_and_wait(
                ManagedService::LandscapeRouter,
                &(|| {
                    manager
                        .active_state(ManagedService::LandscapeRouter)
                        .map(|state| state != "active")
                        .unwrap_or(true)
                }),
            )?;
        } else {
            let accepted = (options.confirm)(
                "stop your Landscape instance with your own process manager, then confirm",
            )?;
            if !accepted {
                return Err(InstallError::UserRefused(
                    "user refused to stop the running instance".into(),
                ));
            }
        }
        super::transaction::mark_phase(root, &transaction, Phase::Activating)?;
        activated = true;
        replace_binary(&current_binary, &new_binary_dir.join(WEBSERVER_BINARY))?;

        if is_systemd {
            manager.start(ManagedService::LandscapeRouter)?;
            super::transaction::mark_phase(root, &transaction, Phase::Verifying)?;
            let pid = manager.main_pid(ManagedService::LandscapeRouter)?;
            if pid == 0 {
                return Err(InstallError::Systemd(
                    "service did not produce a main pid after start".into(),
                ));
            }
            let startup = StartupOptions {
                ports: &options.health.ports,
                expected_pid: pid,
                docs: &options.health.docs,
                unit_state: Some(&(|| manager.active_state(ManagedService::LandscapeRouter).ok())),
                init_required: false,
                data_dir: &root.canonical.join("data"),
                startup_timeout: options.health.startup_timeout,
                stable_duration: options.health.stable_duration,
            };
            health_wait_and_observe(&startup).await?;
        }
        let new_state = pipeline::build_switched_state(
            root,
            &release,
            &built,
            state,
            &transaction.transaction_id,
            unit_sha,
        );
        super::state::write_state(root, &new_state)?;
        super::transaction::mark_phase(root, &transaction, Phase::Committed)?;
        Ok(())
    }
    .await;

    match result {
        Ok(()) => Ok(RepairOutcome::Committed),
        Err(error) if is_systemd && activated => {
            match rollback::rollback_switch(root, &transaction, state, manager, options.health)
                .await
            {
                Ok(()) => Ok(RepairOutcome::RolledBack),
                Err(rollback_error) => {
                    eprintln!(
                        "install: {}",
                        crate::tr!(
                            crate::keys::REPAIR_ROLLBACK_FAILED,
                            rollback_error = rollback_error
                        )
                    );
                    Ok(RepairOutcome::RollbackFailed {
                        reason: error.to_string(),
                    })
                }
            }
        }
        Err(error) => {
            if !activated {
                let _ = super::transaction::mark_phase(root, &transaction, Phase::Failed);
            }
            Err(error)
        }
    }
}

async fn health_wait_and_observe<P: DocsProbe>(
    options: &StartupOptions<'_, P>,
) -> Result<(), InstallError> {
    super::health::wait_for_startup(options).await?;
    super::health::observe_stable(options).await
}

/// 原子替换 `releases/<version>/landscape-webserver`:先写同目录临时文件再 rename。
fn replace_binary(target: &Path, source: &Path) -> Result<(), InstallError> {
    use std::os::unix::fs::PermissionsExt;
    let tmp = target.with_file_name(".landscape-webserver.tmp");
    std::fs::copy(source, &tmp).map_err(InstallError::Io)?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))
        .map_err(InstallError::Io)?;
    std::fs::rename(&tmp, target).map_err(|error| {
        let _ = std::fs::remove_file(&tmp);
        InstallError::Io(error)
    })
}

/// 无 systemd 环境 pending→complete 初始化观测 repair:
/// 不启动/停止进程、不访问端口或 API、不创建 `.lkb`、不改变 active version。
/// 任一步失败时保持旧状态并将事务标为 `failed`。
pub(crate) fn observe_initialization(
    root: &InstallRoot,
    state: &InstallState,
) -> Result<(), InstallError> {
    let transaction = TransactionFile::new_observation_repair(root)?;
    super::transaction::begin(root, &transaction)?;
    let result = (|| {
        let data = root.canonical.join("data");
        pipeline::check_initialization(root, state)?;
        if !data.join("landscape_init.lock").is_file() || !data.join("landscape.toml").is_file() {
            return Err(InstallError::CorruptedState(
                "the initialization lock or persistent config disappeared during the observation repair"
                    .into(),
            ));
        }
        let mut updated = state.clone();
        updated.initialization = super::state::InitializationState {
            status: InitStatus::Complete,
            lock_present: true,
            initialized_at: Some(Utc::now()),
        };
        updated.service.verified = false;
        updated.last_transaction_id = Some(transaction.transaction_id.clone());
        updated.committed_at = Some(Utc::now());
        super::state::write_state(root, &updated)?;
        super::transaction::mark_phase(root, &transaction, Phase::Committed)?;
        Ok(())
    })();
    match result {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = super::transaction::mark_phase(root, &transaction, Phase::Failed);
            Err(error)
        }
    }
}

fn parse_active_version(state: &InstallState) -> Result<semver::Version, InstallError> {
    super::pipeline::parse_stable_version(&state.active_version)
        .map_err(|error| InstallError::CorruptedState(format!("invalid active version: {error}")))
}

fn architecture_from_state(state: &InstallState) -> Architecture {
    match state.assets.webserver.architecture {
        StateArchitecture::X86_64 => Architecture::X86_64,
        StateArchitecture::Aarch64 => Architecture::Aarch64,
    }
}

fn tx_dir(_root: &InstallRoot, transaction: &TransactionFile) -> std::path::PathBuf {
    layout::territory_transactions_dir().join(&transaction.transaction_id)
}

#[cfg(test)]
mod tests {
    use crate::service::systemd::Systemd;
    use std::collections::HashMap;
    use std::os::unix::fs::PermissionsExt;

    use super::super::health::HealthOptions;
    use super::super::repository::ProviderKind;
    use super::super::repository::provider_for;
    use super::super::repository::test_server::{TestResponse, TestServer};
    use super::super::state::{
        ArchiveAsset, Assets, InitializationState, STATE_LAYOUT_VERSION, STATE_SCHEMA_VERSION,
        ServiceState, StateArchitecture, StateServiceManager, WebserverAsset,
    };
    use super::*;

    const TRUSTED_PAYLOAD: &[u8] = b"trusted webserver payload";
    const DRIFTED_PAYLOAD: &[u8] = b"drifted webserver payload";

    /// 建立隔离测试现场:landscape 根与 lkit 地盘并列在临时目录树下,
    /// 地盘由 `test_territory` 指向,返回 (landscape 根, 地盘守卫)。
    fn temp_root(name: &str) -> (std::path::PathBuf, layout::TerritoryOverride) {
        let temp =
            std::env::temp_dir().join(format!("lkit-repair-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();
        let territory = temp.join("territory");
        std::fs::create_dir_all(&territory).unwrap();
        let guard = layout::test_territory(&territory);
        (temp.join("landscape"), guard)
    }

    fn sha256_bytes(bytes: &[u8]) -> (String, u64) {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let hex = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        (hex, bytes.len() as u64)
    }

    fn build_static_zip(html: &str) -> Vec<u8> {
        use std::io::Write;
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("static/index.html", options).unwrap();
        writer.write_all(html.as_bytes()).unwrap();
        writer.finish().unwrap().into_inner()
    }

    fn repository_files_for(version: &str, payload: &[u8], html: &str) -> HashMap<String, Vec<u8>> {
        let webserver_zst = zstd::encode_all(payload, 3).unwrap();
        let (webserver_sha, webserver_size) = sha256_bytes(&webserver_zst);
        let static_zip = build_static_zip(html);
        let (static_sha, static_size) = sha256_bytes(&static_zip);
        let manifest = serde_json::json!({
            "protocol_version": 1,
            "version": version,
            "assets": {
                "webserver": {
                    "x86_64": {
                        "url": "landscape-webserver-x86_64.zst",
                        "sha256": webserver_sha,
                        "size": webserver_size,
                    }
                },
                "static": {
                    "url": "static.zip",
                    "sha256": static_sha,
                    "size": static_size,
                }
            }
        })
        .to_string();
        HashMap::from([
            (
                "/repository.json".into(),
                br#"{"protocol_version":1}"#.to_vec(),
            ),
            (
                "/channels/stable.json".to_string(),
                format!(r#"{{"protocol_version":1,"version":"{version}"}}"#).into_bytes(),
            ),
            (
                format!("/releases/{version}/manifest.json"),
                manifest.into_bytes(),
            ),
            (
                format!("/releases/{version}/landscape-webserver-x86_64.zst"),
                webserver_zst,
            ),
            (format!("/releases/{version}/static.zip"), static_zip),
        ])
    }

    fn start_repository(
        name: &str,
        files: HashMap<String, Vec<u8>>,
    ) -> (
        TestServer,
        InstallRoot,
        ReleaseProvider,
        layout::TerritoryOverride,
    ) {
        let server = TestServer::start(move |path| match files.get(path) {
            Some(body) => TestResponse::ok(body.clone()),
            None => TestResponse::status(404, "Not Found", Vec::new()),
        });
        let (root, guard) = temp_root(name);
        let install_root = InstallRoot {
            install_root: root.clone(),
            canonical: root,
        };
        let provider = provider_for(ProviderKind::Http, &server.base).unwrap();
        (server, install_root, provider, guard)
    }

    fn install_state(
        root: &InstallRoot,
        manager: StateServiceManager,
        init_status: InitStatus,
        static_sha: &str,
        static_size: u64,
    ) -> InstallState {
        let (webserver_sha, webserver_size) = sha256_bytes(TRUSTED_PAYLOAD);
        InstallState {
            schema_version: STATE_SCHEMA_VERSION,
            layout_version: STATE_LAYOUT_VERSION,
            install_root: root.install_root.display().to_string(),
            canonical_install_root: root.canonical.display().to_string(),
            active_version: "1.2.3".into(),
            assets: Assets {
                webserver: WebserverAsset {
                    architecture: StateArchitecture::X86_64,
                    sha256: webserver_sha,
                    size: webserver_size,
                },
                static_archive: ArchiveAsset {
                    sha256: static_sha.into(),
                    size: static_size,
                },
            },
            initialization: InitializationState {
                status: init_status,
                lock_present: init_status == InitStatus::Complete,
                initialized_at: (init_status == InitStatus::Complete).then(chrono::Utc::now),
            },
            service: ServiceState {
                manager,
                registered: manager == StateServiceManager::Systemd,
                enabled: manager == StateServiceManager::Systemd,
                verified: manager == StateServiceManager::Systemd,
                definition_path: (manager == StateServiceManager::Systemd)
                    .then(|| "service/landscape-router.service".into()),
                definition_sha256: (manager == StateServiceManager::Systemd)
                    .then(|| "d".repeat(64)),
            },
            last_transaction_id: None,
            committed_at: Some(chrono::Utc::now()),
        }
    }

    struct FakeDocs;

    impl DocsProbe for FakeDocs {
        async fn docs_ok(&self) -> bool {
            true
        }
    }

    struct FailingDocs;

    impl DocsProbe for FailingDocs {
        async fn docs_ok(&self) -> bool {
            false
        }
    }

    /// 以落盘二进制作为阶段信号:激活验证期间二进制是修复后的可信内容
    /// (探测失败),回滚从 `.lkb` 重建后恢复为漂移内容(探测成功)。
    /// 事件驱动,不依赖墙钟,消除并行负载下的时序竞态。
    struct RepairedBinaryDocs {
        binary: std::path::PathBuf,
    }

    impl DocsProbe for RepairedBinaryDocs {
        async fn docs_ok(&self) -> bool {
            std::fs::read(&self.binary)
                .map(|bytes| bytes == DRIFTED_PAYLOAD)
                .unwrap_or(false)
        }
    }

    fn none_health() -> HealthOptions<FakeDocs> {
        HealthOptions {
            docs: FakeDocs,
            ports: Vec::new(),
            startup_timeout: std::time::Duration::from_secs(5),
            stable_duration: std::time::Duration::from_millis(100),
        }
    }

    fn failing_health<P: DocsProbe>(docs: P) -> HealthOptions<P> {
        HealthOptions {
            docs,
            ports: Vec::new(),
            startup_timeout: std::time::Duration::from_secs(2),
            stable_duration: std::time::Duration::from_millis(100),
        }
    }

    /// 初始化 watcher:模拟运行中的后端在 `landscape_init.toml` 落盘后
    /// 创建 `landscape_init.lock` 与 `landscape.toml`(回滚健康检查要求)。
    fn init_watcher(
        data_dir: std::path::PathBuf,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) {
        let watcher_stop = stop.clone();
        std::thread::spawn(move || {
            while !watcher_stop.load(std::sync::atomic::Ordering::Relaxed) {
                if data_dir.join("landscape_init.toml").is_file() {
                    let _ = std::fs::write(data_dir.join("landscape_init.lock"), b"");
                    let _ = std::fs::write(data_dir.join("landscape.toml"), b"");
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        });
    }

    static YES: fn(&str) -> Result<bool, InstallError> = |_| Ok(true);

    fn activate_version(root: &InstallRoot, version: &str) {
        let release = root.canonical.join("releases").join(version);
        std::fs::create_dir_all(release.join("static")).unwrap();
        std::fs::write(release.join("static/index.html"), "<h1>default</h1>").unwrap();
        let _ = std::fs::remove_file(root.canonical.join("current"));
        std::os::unix::fs::symlink(
            format!("releases/{version}"),
            root.canonical.join("current"),
        )
        .unwrap();
    }

    fn load_transaction_json() -> serde_json::Value {
        let entries: Vec<_> = std::fs::read_dir(layout::territory_transactions_dir())
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        let path = entries
            .iter()
            .map(|entry| entry.path())
            .find(|path| path.extension().is_some_and(|ext| ext == "json"))
            .expect("the transaction json must exist");
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    #[tokio::test]
    async fn restores_static_pages_from_the_repository() {
        let static_zip = build_static_zip("<h1>new</h1>");
        let (static_sha, static_size) = sha256_bytes(&static_zip);
        let files = repository_files_for("1.2.3", TRUSTED_PAYLOAD, "<h1>new</h1>");
        let (_server, root, provider, _guard) = start_repository("static-ok", files);
        activate_version(&root, "1.2.3");
        std::fs::write(
            root.canonical.join("current/static/index.html"),
            "<h1>old</h1>",
        )
        .unwrap();
        let state = install_state(
            &root,
            StateServiceManager::Systemd,
            InitStatus::Complete,
            &static_sha,
            static_size,
        );

        repair_static(&root, &provider, &state, false)
            .await
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(root.canonical.join("current/static/index.html")).unwrap(),
            "<h1>new</h1>"
        );
        let entries: Vec<_> = std::fs::read_dir(layout::territory_transactions_dir())
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(
            entries.len(),
            2,
            "transaction json plus its diagnostics directory"
        );
        let tx_dir = entries
            .iter()
            .find(|entry| entry.path().is_dir())
            .expect("the transaction diagnostics directory must exist")
            .path();
        assert!(
            tx_dir.join("static-backup/index.html").is_file(),
            "static backup must preserve the previous pages"
        );
        assert!(
            !layout::territory_config_file().exists(),
            "repair must not create config.toml"
        );
        let _ = std::fs::remove_dir_all(root.install_root.parent().unwrap());
    }

    #[tokio::test]
    async fn static_repair_commits_without_any_runtime_check_for_systemd_too() {
        let static_zip = build_static_zip("<h1>new</h1>");
        let (static_sha, static_size) = sha256_bytes(&static_zip);
        let files = repository_files_for("1.2.3", TRUSTED_PAYLOAD, "<h1>new</h1>");
        let (_server, root, provider, _guard) = start_repository("static-systemd", files);
        activate_version(&root, "1.2.3");
        std::fs::write(
            root.canonical.join("current/static/index.html"),
            "<h1>old</h1>",
        )
        .unwrap();
        let state = install_state(
            &root,
            StateServiceManager::Systemd,
            InitStatus::Complete,
            &static_sha,
            static_size,
        );

        // systemd 管理下的安装同样只做纯文件替换,不探测 /api/docs、不重启服务。
        repair_static(&root, &provider, &state, false)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(root.canonical.join("current/static/index.html")).unwrap(),
            "<h1>new</h1>"
        );
        let updated = super::super::state::load_state(&root).unwrap().unwrap();
        assert_eq!(updated.service.manager, StateServiceManager::Systemd);
        assert!(updated.service.verified);
        assert!(
            !layout::territory_backups_dir().exists(),
            "static repair must not create a .lkb backup"
        );
        let _ = std::fs::remove_dir_all(root.install_root.parent().unwrap());
    }

    #[tokio::test]
    async fn repairs_static_from_a_repository_with_different_assets() {
        let files = repository_files_for("1.2.3", TRUSTED_PAYLOAD, "<h1>other</h1>");
        let (_server, root, provider, _guard) = start_repository("static-mismatch", files);
        activate_version(&root, "1.2.3");
        let state = install_state(
            &root,
            StateServiceManager::Systemd,
            InitStatus::Complete,
            &"e".repeat(64),
            99,
        );

        // 记录身份与仓库不一致时,repair 仍以仓库为准恢复官方页面,并更新
        // state 身份、刷新版本目录 static.zip。
        repair_static(&root, &provider, &state, false)
            .await
            .unwrap();
        let updated = crate::deployment::state::load_state(&root)
            .unwrap()
            .unwrap();
        let release = provider
            .release(
                &semver::Version::new(1, 2, 3),
                super::super::repository::Architecture::X86_64,
            )
            .await
            .unwrap();
        assert_eq!(
            updated.assets.static_archive.sha256, release.assets.static_archive.sha256,
            "the state identity must be updated to the repository's official asset"
        );
        assert_eq!(
            std::fs::read(root.canonical.join("current/static/index.html")).unwrap(),
            b"<h1>other</h1>"
        );
        let (refreshed_sha, _) =
            sha256_bytes(&std::fs::read(root.canonical.join("releases/1.2.3/static.zip")).unwrap());
        assert_eq!(
            refreshed_sha, release.assets.static_archive.sha256,
            "the version dir static.zip must be refreshed with the official asset"
        );
        let _ = std::fs::remove_dir_all(root.install_root.parent().unwrap());
    }

    /// e2e 契约:显式 `--repository` 的 `repair static` 在 config.toml 损坏时仍能
    /// 恢复官方页面——前端源解析是宽容的,损坏配置按"未配置自定义前端"处理。
    #[tokio::test]
    async fn static_repair_falls_back_to_official_pages_when_the_config_is_corrupted() {
        let files = repository_files_for("1.2.3", TRUSTED_PAYLOAD, "<h1>official</h1>");
        let (_server, root, provider, _guard) = start_repository("corrupt-config-repair", files);
        activate_version(&root, "1.2.3");
        std::fs::write(
            root.canonical.join("current/static/index.html"),
            "<h1>customized</h1>",
        )
        .unwrap();
        let state = install_state(
            &root,
            StateServiceManager::Systemd,
            InitStatus::Complete,
            &"a".repeat(64),
            1,
        );
        std::fs::write(layout::territory_config_file(), b"not valid toml [[[").unwrap();

        repair_static(&root, &provider, &state, false)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(root.canonical.join("current/static/index.html")).unwrap(),
            "<h1>official</h1>",
            "a corrupted config must fall back to the official static repair"
        );
        let _ = std::fs::remove_dir_all(root.install_root.parent().unwrap());
    }

    /// 前端源 TestServer:stable 通道 + 只声明 static 的 manifest + static.zip。
    fn frontend_server(html: &str) -> (TestServer, String, (String, u64)) {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        zip.start_file(
            "static/index.html",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(html.as_bytes()).unwrap();
        let zip_bytes = zip.finish().unwrap().into_inner();
        let (sha, size) = sha256_bytes(&zip_bytes);
        let files: HashMap<String, Vec<u8>> = HashMap::from([
            ("/repository.json".into(), br#"{"protocol_version":1}"#.to_vec()),
            (
                "/channels/stable.json".into(),
                br#"{"protocol_version":1,"version":"1.0.0"}"#.to_vec(),
            ),
            (
                "/releases/1.0.0/manifest.json".into(),
                format!(
                    r#"{{"protocol_version":1,"version":"1.0.0","assets":{{"webserver":{{}},"static":{{"url":"static.zip","sha256":"{sha}","size":{size}}}}}}}"#
                )
                .into_bytes(),
            ),
            ("/releases/1.0.0/static.zip".into(), zip_bytes),
        ]);
        let server = TestServer::start(move |path| match files.get(path) {
            Some(body) => TestResponse::ok(body.clone()),
            None => TestResponse::status(404, "Not Found", Vec::new()),
        });
        let location = server.base.clone();
        (server, location, (sha, size))
    }

    #[tokio::test]
    async fn static_repair_restores_the_configured_custom_frontend() {
        let files = repository_files_for("1.2.3", TRUSTED_PAYLOAD, "<h1>official</h1>");
        let (_server, root, provider, _guard) = start_repository("custom-repair", files);
        activate_version(&root, "1.2.3");
        std::fs::write(
            root.canonical.join("current/static/index.html"),
            "<h1>official</h1>",
        )
        .unwrap();
        let state = install_state(
            &root,
            StateServiceManager::Systemd,
            InitStatus::Complete,
            &"a".repeat(64),
            1,
        );
        let (frontend_server, location, _) = frontend_server("<h1>custom</h1>");
        let _ = frontend_server;
        std::fs::write(
            layout::territory_config_file(),
            format!(
                "schema_version = 1\n\n[repository]\nkind = \"github\"\nlocation = \"ThisSeanZhang/landscape\"\n\n[frontend]\nactive = \"custom\"\n\n[[frontend.sources]]\nid = \"custom\"\nkind = \"http\"\nlocation = \"{location}\"\n"
            ),
        )
        .unwrap();

        // 激活自定义前端源时,repair static 恢复自定义前端;state 身份保持原样。
        super::super::state::write_state(&root, &state).unwrap();
        let state_before = crate::deployment::state::load_state(&root)
            .unwrap()
            .unwrap();
        repair_static(&root, &provider, &state, false)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(root.canonical.join("current/static/index.html")).unwrap(),
            "<h1>custom</h1>"
        );
        let updated = crate::deployment::state::load_state(&root)
            .unwrap()
            .unwrap();
        assert_eq!(
            updated.assets.static_archive, state_before.assets.static_archive,
            "custom repair must not touch the official zip identity"
        );
        assert!(
            !root.canonical.join("releases/1.2.3/static.zip").exists(),
            "custom repair must not touch the version dir static.zip (official baseline only)"
        );
        let _ = std::fs::remove_dir_all(root.install_root.parent().unwrap());
    }

    #[tokio::test]
    async fn static_repair_official_forces_the_official_pages() {
        let files = repository_files_for("1.2.3", TRUSTED_PAYLOAD, "<h1>official</h1>");
        let (_server, root, provider, _guard) = start_repository("official-repair", files);
        activate_version(&root, "1.2.3");
        std::fs::write(
            root.canonical.join("current/static/index.html"),
            "<h1>customized</h1>",
        )
        .unwrap();
        let state = install_state(
            &root,
            StateServiceManager::Systemd,
            InitStatus::Complete,
            &"a".repeat(64),
            1,
        );
        let (frontend_server, location, _) = frontend_server("<h1>custom</h1>");
        let _ = frontend_server;
        std::fs::write(
            layout::territory_config_file(),
            format!(
                "schema_version = 1\n\n[repository]\nkind = \"github\"\nlocation = \"ThisSeanZhang/landscape\"\n\n[frontend]\nactive = \"custom\"\n\n[[frontend.sources]]\nid = \"custom\"\nkind = \"http\"\nlocation = \"{location}\"\n"
            ),
        )
        .unwrap();

        // --official 无条件恢复官方页面并更新身份。
        repair_static(&root, &provider, &state, true).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(root.canonical.join("current/static/index.html")).unwrap(),
            "<h1>official</h1>"
        );
        let updated = crate::deployment::state::load_state(&root)
            .unwrap()
            .unwrap();
        let release = provider
            .release(
                &semver::Version::new(1, 2, 3),
                super::super::repository::Architecture::X86_64,
            )
            .await
            .unwrap();
        assert_eq!(
            updated.assets.static_archive.sha256,
            release.assets.static_archive.sha256
        );
        let _ = std::fs::remove_dir_all(root.install_root.parent().unwrap());
    }

    #[tokio::test]
    async fn repairs_drifted_backend_with_systemd() {
        let static_zip = build_static_zip("<h1>page</h1>");
        let (static_sha, static_size) = sha256_bytes(&static_zip);
        let files = repository_files_for("1.2.3", TRUSTED_PAYLOAD, "<h1>page</h1>");
        let server = TestServer::start(move |path| {
            match path {
            "/api/v1/system/config/export" => TestResponse::ok(
                br#"{"data":{"filename":"landscape_init_v1.2.3.toml","version":"1.2.3","content":"version = \"1.2.3\"\n"}}"#
                    .to_vec(),
            ),
            other => match files.get(other) {
                Some(body) => TestResponse::ok(body.clone()),
                None => TestResponse::status(404, "Not Found", Vec::new()),
            },
        }
        });
        let (root, _guard) = temp_root("binary-repair");
        let install_root = InstallRoot {
            install_root: root.clone(),
            canonical: root.clone(),
        };
        let provider = provider_for(ProviderKind::Http, &server.base).unwrap();
        activate_version(&install_root, "1.2.3");
        // systemd 假环境:stop/start 维护 state 文件,MainPID 指向测试进程。
        let fake_dir = root.join("fake-systemd");
        std::fs::create_dir_all(fake_dir.join("units")).unwrap();
        std::fs::create_dir_all(fake_dir.join("run")).unwrap();
        std::fs::write(fake_dir.join("state"), b"active").unwrap();
        let script = fake_dir.join("systemctl");
        std::fs::write(
            &script,
            format!(
                r#"#!/bin/sh
STATE_FILE="{}"
case "$*" in
  "start landscape-router.service") echo active > "$STATE_FILE"; exit 0;;
  "stop landscape-router.service") echo inactive > "$STATE_FILE"; exit 0;;
  "show --property=ActiveState --value landscape-router.service") cat "$STATE_FILE";;
  "show --property=MainPID --value landscape-router.service") echo {};;
  "is-enabled landscape-router.service") echo enabled;;
  "is-active landscape-router.service") cat "$STATE_FILE";;
  *) exit 0;;
esac
"#,
                fake_dir.join("state").display(),
                std::process::id()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let systemd = Systemd {
            systemctl: script,
            system_unit_dir: fake_dir.join("units"),
            run_systemd_dir: fake_dir.join("run"),
            pid1_is_systemd: true,
            resolv_conf: fake_dir.join("resolv.conf"),
        };
        std::fs::create_dir_all(install_root.canonical.join("service")).unwrap();
        std::fs::write(
            install_root
                .canonical
                .join("service/landscape-router.service"),
            b"[Unit]\nDescription=Landscape Router\n",
        )
        .unwrap();
        std::fs::write(
            install_root.canonical.join("releases/1.2.3/static.zip"),
            &static_zip,
        )
        .unwrap();
        let binary = install_root
            .canonical
            .join("releases/1.2.3/landscape-webserver");
        std::fs::write(&binary, DRIFTED_PAYLOAD).unwrap();
        std::fs::create_dir_all(install_root.canonical.join("data")).unwrap();
        std::fs::write(install_root.canonical.join("data/landscape_init.lock"), b"").unwrap();
        std::fs::write(install_root.canonical.join("data/landscape.toml"), b"").unwrap();
        let state = install_state(
            &install_root,
            StateServiceManager::Systemd,
            InitStatus::Complete,
            &static_sha,
            static_size,
        );

        static TOKEN: fn() -> Result<String, InstallError> = || Ok("tok".into());
        let options = SwitchOptions {
            export_base_url: server.base.clone(),
            token: &TOKEN,
            confirm: &YES,
            health: &none_health(),
        };
        let outcome = repair_binary(&install_root, &provider, &state, &systemd, &options)
            .await
            .unwrap();
        assert!(matches!(outcome, RepairOutcome::Committed));

        assert_eq!(std::fs::read(&binary).unwrap(), TRUSTED_PAYLOAD);
        let entries: Vec<_> = std::fs::read_dir(layout::territory_transactions_dir())
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        let tx_dir = entries
            .iter()
            .find(|entry| entry.path().is_dir())
            .expect("the transaction diagnostics directory must exist")
            .path();
        assert_eq!(
            std::fs::read(tx_dir.join("repaired-binary")).unwrap(),
            DRIFTED_PAYLOAD,
            "the pre-repair binary must be preserved in the transaction diagnostics directory"
        );
        assert!(layout::territory_backups_dir().read_dir().unwrap().count() >= 2);
        let state = super::super::state::load_state(&install_root)
            .unwrap()
            .unwrap();
        let (expected_sha, _) = sha256_bytes(TRUSTED_PAYLOAD);
        assert_eq!(state.assets.webserver.sha256, expected_sha);
        assert!(
            super::super::transaction::find_unfinished(&install_root)
                .unwrap()
                .is_none()
        );
        assert!(
            !layout::territory_config_file().exists(),
            "repair must not create config.toml"
        );
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[tokio::test]
    async fn binary_repair_rolls_back_when_activation_fails() {
        let static_zip = build_static_zip("<h1>page</h1>");
        let (static_sha, static_size) = sha256_bytes(&static_zip);
        let files = repository_files_for("1.2.3", TRUSTED_PAYLOAD, "<h1>page</h1>");
        let server = TestServer::start(move |path| {
            match path {
            "/api/v1/system/config/export" => TestResponse::ok(
                br#"{"data":{"filename":"landscape_init_v1.2.3.toml","version":"1.2.3","content":"version = \"1.2.3\"\n"}}"#
                    .to_vec(),
            ),
            other => match files.get(other) {
                Some(body) => TestResponse::ok(body.clone()),
                None => TestResponse::status(404, "Not Found", Vec::new()),
            },
        }
        });
        let (root, _guard) = temp_root("binary-repair-rollback");
        let install_root = InstallRoot {
            install_root: root.clone(),
            canonical: root.clone(),
        };
        let provider = provider_for(ProviderKind::Http, &server.base).unwrap();
        activate_version(&install_root, "1.2.3");
        let fake_dir = root.join("fake-systemd");
        std::fs::create_dir_all(fake_dir.join("units")).unwrap();
        std::fs::create_dir_all(fake_dir.join("run")).unwrap();
        std::fs::write(fake_dir.join("state"), b"active").unwrap();
        let script = fake_dir.join("systemctl");
        std::fs::write(
            &script,
            format!(
                r#"#!/bin/sh
STATE_FILE="{}"
case "$*" in
  "start landscape-router.service") echo active > "$STATE_FILE"; exit 0;;
  "stop landscape-router.service") echo inactive > "$STATE_FILE"; exit 0;;
  "show --property=ActiveState --value landscape-router.service") cat "$STATE_FILE";;
  "show --property=MainPID --value landscape-router.service") echo {};;
  "is-enabled landscape-router.service") echo enabled;;
  "is-active landscape-router.service") cat "$STATE_FILE";;
  *) exit 0;;
esac
"#,
                fake_dir.join("state").display(),
                std::process::id()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let systemd = Systemd {
            systemctl: script,
            system_unit_dir: fake_dir.join("units"),
            run_systemd_dir: fake_dir.join("run"),
            pid1_is_systemd: true,
            resolv_conf: fake_dir.join("resolv.conf"),
        };
        std::fs::create_dir_all(install_root.canonical.join("service")).unwrap();
        std::fs::write(
            install_root
                .canonical
                .join("service/landscape-router.service"),
            b"[Unit]\nDescription=Landscape Router\n",
        )
        .unwrap();
        std::fs::write(
            install_root.canonical.join("releases/1.2.3/static.zip"),
            &static_zip,
        )
        .unwrap();
        let binary = install_root
            .canonical
            .join("releases/1.2.3/landscape-webserver");
        std::fs::write(&binary, DRIFTED_PAYLOAD).unwrap();
        std::fs::create_dir_all(install_root.canonical.join("data")).unwrap();
        std::fs::write(install_root.canonical.join("data/landscape_init.lock"), b"").unwrap();
        std::fs::write(install_root.canonical.join("data/landscape.toml"), b"").unwrap();
        let state = install_state(
            &install_root,
            StateServiceManager::Systemd,
            InitStatus::Complete,
            &static_sha,
            static_size,
        );
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        init_watcher(install_root.canonical.join("data"), stop.clone());
        // 修复前 `.lkb` 保留漂移二进制;激活验证失败后回滚从 `.lkb` 重建,
        // 探测恢复为漂移内容后通过,与 `repairs_drifted_backend_with_systemd`
        // 共用修复前二进制存档断言。
        let health = failing_health(RepairedBinaryDocs {
            binary: binary.clone(),
        });
        static TOKEN: fn() -> Result<String, InstallError> = || Ok("tok".into());
        let options = SwitchOptions {
            export_base_url: server.base.clone(),
            token: &TOKEN,
            confirm: &YES,
            health: &health,
        };
        let outcome = repair_binary(&install_root, &provider, &state, &systemd, &options)
            .await
            .unwrap();
        assert!(
            matches!(outcome, RepairOutcome::RolledBack),
            "expected rolled back repair, got {outcome:?}"
        );

        assert_eq!(
            std::fs::read(&binary).unwrap(),
            DRIFTED_PAYLOAD,
            "the pre-repair binary must be restored from the .lkb"
        );
        assert_eq!(
            std::fs::read_link(install_root.canonical.join("current")).unwrap(),
            std::path::PathBuf::from("releases/1.2.3")
        );
        let state = super::super::state::load_state(&install_root)
            .unwrap()
            .unwrap();
        let (drifted_sha, _) = sha256_bytes(DRIFTED_PAYLOAD);
        assert_eq!(state.assets.webserver.sha256, drifted_sha);
        let tx = load_transaction_json();
        assert_eq!(tx["phase"], "rolled_back");
        assert_eq!(tx["operation"], "repair");
        assert!(tx["backup"].is_object());
        assert!(
            super::super::transaction::find_unfinished(&install_root)
                .unwrap()
                .is_none()
        );
        assert!(
            !layout::territory_config_file().exists(),
            "repair must not create config.toml"
        );
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[tokio::test]
    async fn binary_repair_rollback_failure_returns_rollback_failed() {
        let static_zip = build_static_zip("<h1>page</h1>");
        let (static_sha, static_size) = sha256_bytes(&static_zip);
        let files = repository_files_for("1.2.3", TRUSTED_PAYLOAD, "<h1>page</h1>");
        let server = TestServer::start(move |path| {
            match path {
            "/api/v1/system/config/export" => TestResponse::ok(
                br#"{"data":{"filename":"landscape_init_v1.2.3.toml","version":"1.2.3","content":"version = \"1.2.3\"\n"}}"#
                    .to_vec(),
            ),
            other => match files.get(other) {
                Some(body) => TestResponse::ok(body.clone()),
                None => TestResponse::status(404, "Not Found", Vec::new()),
            },
        }
        });
        let (root, _guard) = temp_root("binary-repair-rollback-failed");
        let install_root = InstallRoot {
            install_root: root.clone(),
            canonical: root.clone(),
        };
        let provider = provider_for(ProviderKind::Http, &server.base).unwrap();
        activate_version(&install_root, "1.2.3");
        let fake_dir = root.join("fake-systemd");
        std::fs::create_dir_all(fake_dir.join("units")).unwrap();
        std::fs::create_dir_all(fake_dir.join("run")).unwrap();
        std::fs::write(fake_dir.join("state"), b"active").unwrap();
        let script = fake_dir.join("systemctl");
        std::fs::write(
            &script,
            format!(
                r#"#!/bin/sh
STATE_FILE="{}"
case "$*" in
  "start landscape-router.service") echo active > "$STATE_FILE"; exit 0;;
  "stop landscape-router.service") echo inactive > "$STATE_FILE"; exit 0;;
  "show --property=ActiveState --value landscape-router.service") cat "$STATE_FILE";;
  "show --property=MainPID --value landscape-router.service") echo {};;
  "is-enabled landscape-router.service") echo enabled;;
  "is-active landscape-router.service") cat "$STATE_FILE";;
  *) exit 0;;
esac
"#,
                fake_dir.join("state").display(),
                std::process::id()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let systemd = Systemd {
            systemctl: script,
            system_unit_dir: fake_dir.join("units"),
            run_systemd_dir: fake_dir.join("run"),
            pid1_is_systemd: true,
            resolv_conf: fake_dir.join("resolv.conf"),
        };
        std::fs::create_dir_all(install_root.canonical.join("service")).unwrap();
        std::fs::write(
            install_root
                .canonical
                .join("service/landscape-router.service"),
            b"[Unit]\nDescription=Landscape Router\n",
        )
        .unwrap();
        std::fs::write(
            install_root.canonical.join("releases/1.2.3/static.zip"),
            &static_zip,
        )
        .unwrap();
        let binary = install_root
            .canonical
            .join("releases/1.2.3/landscape-webserver");
        std::fs::write(&binary, DRIFTED_PAYLOAD).unwrap();
        std::fs::create_dir_all(install_root.canonical.join("data")).unwrap();
        std::fs::write(install_root.canonical.join("data/landscape_init.lock"), b"").unwrap();
        std::fs::write(install_root.canonical.join("data/landscape.toml"), b"").unwrap();
        let state = install_state(
            &install_root,
            StateServiceManager::Systemd,
            InitStatus::Complete,
            &static_sha,
            static_size,
        );
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        init_watcher(install_root.canonical.join("data"), stop.clone());
        // 探测恒失败:激活验证失败触发回滚,回滚自身的健康检查同样失败,
        // 返回人工恢复结果;事务保持 `failed` 且诊断目录保留。
        let health = failing_health(FailingDocs);
        static TOKEN: fn() -> Result<String, InstallError> = || Ok("tok".into());
        let options = SwitchOptions {
            export_base_url: server.base.clone(),
            token: &TOKEN,
            confirm: &YES,
            health: &health,
        };
        let outcome = repair_binary(&install_root, &provider, &state, &systemd, &options)
            .await
            .unwrap();
        let RepairOutcome::RollbackFailed { reason } = outcome else {
            panic!("expected rollback failed repair, got {outcome:?}");
        };
        assert!(!reason.is_empty());

        let tx = load_transaction_json();
        assert_eq!(
            tx["phase"], "failed",
            "a failed rollback must leave the transaction in the failed phase"
        );
        assert_eq!(tx["operation"], "repair");
        let tx_dir =
            layout::territory_transactions_dir().join(tx["transaction_id"].as_str().unwrap());
        assert!(
            tx_dir.join("failed-data").is_dir(),
            "the interrupted data must be preserved for manual recovery"
        );
        assert!(
            tx_dir.join("replaced-release").is_dir(),
            "the repaired release must be preserved for manual recovery"
        );
        assert!(
            tx_dir.join("repaired-binary").is_file(),
            "the pre-repair binary must be preserved for manual recovery"
        );
        assert_eq!(
            std::fs::read(&binary).unwrap(),
            DRIFTED_PAYLOAD,
            "the release rebuilt from the .lkb stays in place"
        );
        assert!(
            super::super::transaction::find_unfinished(&install_root)
                .unwrap()
                .is_none()
        );
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn observes_pending_initialization_as_complete() {
        let (root, _guard) = temp_root("observe");
        let install_root = InstallRoot {
            install_root: root.clone(),
            canonical: root.clone(),
        };
        activate_version(&install_root, "1.2.3");
        let state = install_state(
            &install_root,
            StateServiceManager::Systemd,
            InitStatus::Pending,
            &"b".repeat(64),
            1,
        );
        std::fs::create_dir_all(install_root.canonical.join("data")).unwrap();
        std::fs::write(
            install_root.canonical.join("data/landscape_init.toml"),
            b"version = \"1.2.3\"\n",
        )
        .unwrap();
        std::fs::write(install_root.canonical.join("data/landscape_init.lock"), b"").unwrap();
        std::fs::write(install_root.canonical.join("data/landscape.toml"), b"").unwrap();

        observe_initialization(&install_root, &state).unwrap();

        assert!(
            layout::territory_state_path().is_file(),
            "the install state must live in the lkit territory"
        );
        assert!(
            !install_root.canonical.join("state").exists(),
            "the landscape root must not hold lkit metadata"
        );
        let updated = super::super::state::load_state(&install_root)
            .unwrap()
            .unwrap();
        assert_eq!(updated.initialization.status, InitStatus::Complete);
        assert!(updated.initialization.lock_present);
        assert!(updated.initialization.initialized_at.is_some());
        assert!(!updated.service.verified);
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn observation_repair_fails_when_lock_disappears() {
        let (root, _guard) = temp_root("observe-missing");
        let install_root = InstallRoot {
            install_root: root.clone(),
            canonical: root.clone(),
        };
        activate_version(&install_root, "1.2.3");
        let state = install_state(
            &install_root,
            StateServiceManager::Systemd,
            InitStatus::Pending,
            &"b".repeat(64),
            1,
        );
        std::fs::create_dir_all(install_root.canonical.join("data")).unwrap();
        std::fs::write(install_root.canonical.join("data/landscape.toml"), b"").unwrap();
        super::super::state::write_state(&install_root, &state).unwrap();

        assert!(observe_initialization(&install_root, &state).is_err());
        let updated = super::super::state::load_state(&install_root)
            .unwrap()
            .unwrap();
        assert_eq!(updated.initialization.status, InitStatus::Pending);
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }
}
