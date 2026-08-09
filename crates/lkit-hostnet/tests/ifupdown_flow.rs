//! ifupdown 适配器全流程集成测试:
//! 收集 → 备份 → 改写 → 校验 → 恢复,以及校验失败后的恢复路径。

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use lkit_hostnet::FileSources;
use lkit_hostnet::{
    EditOutcome, EditPlan, FileSet, HostNetError, HostNetworkAdapter, Manifest, ToolPaths,
    Validation, ifupdown::IfupdownAdapter,
};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lkit-hostnet-flow-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn fake_ifup(root: &Path, exit_code: i32) -> PathBuf {
    let tool = root.join("fake-ifup");
    std::fs::write(
        &tool,
        format!(
            "#!/bin/sh\n[ \"$#\" -eq 3 ] || exit 64\n[ \"$1\" = \"--no-act\" ] || exit 64\ncase \"$2\" in --interfaces=*) ;; *) exit 64;; esac\n[ \"$3\" = \"--all\" ] || exit 64\nexit {exit_code}\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
    tool
}

struct Rootfs {
    dir: PathBuf,
    interfaces: PathBuf,
    fragment: PathBuf,
    original_main: Vec<u8>,
    original_fragment: Vec<u8>,
}

struct ExternalChangeAdapter {
    inner: IfupdownAdapter,
    path: PathBuf,
}

impl HostNetworkAdapter for ExternalChangeAdapter {
    fn collect(&self, sources: &FileSources) -> Result<FileSet, HostNetError> {
        self.inner.collect(sources)
    }

    fn plan_unmanage(
        &self,
        file_set: &FileSet,
        selected: &[String],
    ) -> Result<EditPlan, HostNetError> {
        self.inner.plan_unmanage(file_set, selected)
    }

    fn apply(&self, _plan: &EditPlan) -> Result<EditOutcome, HostNetError> {
        std::fs::write(&self.path, b"changed by another process\n").unwrap();
        Err(HostNetError::ConcurrentModification {
            path: self.path.clone(),
        })
    }

    fn backup(&self, plan: &EditPlan, dest: &Path) -> Result<Manifest, HostNetError> {
        self.inner.backup(plan, dest)
    }

    fn restore(&self, manifest: &Manifest) -> Result<(), HostNetError> {
        self.inner.restore(manifest)
    }

    fn restore_if_unchanged(
        &self,
        manifest: &Manifest,
        plan: &EditPlan,
    ) -> Result<(), HostNetError> {
        self.inner.restore_if_unchanged(manifest, plan)
    }

    fn validate(&self, file_set: &FileSet, tools: &ToolPaths) -> Result<Validation, HostNetError> {
        self.inner.validate(file_set, tools)
    }
}

fn rootfs(name: &str) -> Rootfs {
    let dir = temp_dir(name);
    let fragments = dir.join("interfaces.d");
    std::fs::create_dir_all(&fragments).unwrap();
    let interfaces = dir.join("interfaces");
    let original_main = b"# managed by ifupdown\nauto eth0\niface eth0 inet static\n    address 198.51.100.20\n    gateway 198.51.100.1\nauto lo\niface lo inet loopback\nsource interfaces.d/*\n";
    std::fs::write(&interfaces, original_main).unwrap();
    let fragment = fragments.join("lan.conf");
    let original_fragment = b"auto eth1\niface eth1 inet dhcp\n    hostname router\n";
    std::fs::write(&fragment, original_fragment).unwrap();
    Rootfs {
        dir,
        interfaces,
        fragment,
        original_main: original_main.to_vec(),
        original_fragment: original_fragment.to_vec(),
    }
}

#[test]
fn full_flow_unmanages_and_restores_verbatim() {
    let fs = rootfs("full");
    let adapter = IfupdownAdapter::new();
    let sources = FileSources::new(fs.interfaces.clone());
    let file_set = adapter.collect(&sources).unwrap();
    assert_eq!(file_set.files.len(), 2);

    let plan = adapter
        .plan_unmanage(&file_set, &["eth0".into(), "eth1".into()])
        .unwrap();
    assert_eq!(plan.edits.len(), 2);

    let backup_dir = fs.dir.join("backup");
    let manifest = adapter.backup(&plan, &backup_dir).unwrap();
    assert!(backup_dir.join("manifest.json").is_file());

    let outcome = adapter.apply(&plan).unwrap();
    assert_eq!(outcome.edited.len(), 2);

    let main = std::fs::read_to_string(&fs.interfaces).unwrap();
    assert!(main.contains("iface eth0 inet manual"));
    assert!(!main.contains("address 198.51.100.20"));
    assert!(!main.contains("auto eth0"));
    assert!(main.contains("iface lo inet loopback"));

    let fragment = std::fs::read_to_string(&fs.fragment).unwrap();
    assert!(fragment.contains("iface eth1 inet manual"));
    assert!(!fragment.contains("hostname router"));

    let tools = ToolPaths {
        ifup: Some(fake_ifup(&fs.dir, 0)),
    };
    assert_eq!(
        adapter.validate(&file_set, &tools).unwrap(),
        Validation::Clean
    );

    adapter.restore(&manifest).unwrap();
    assert_eq!(std::fs::read(&fs.interfaces).unwrap(), fs.original_main);
    assert_eq!(std::fs::read(&fs.fragment).unwrap(), fs.original_fragment);
}

#[test]
fn validation_failure_then_restore_leaves_original_content() {
    let fs = rootfs("failure");
    let adapter = IfupdownAdapter::new();
    let file_set = adapter
        .collect(&FileSources::new(fs.interfaces.clone()))
        .unwrap();
    let plan = adapter.plan_unmanage(&file_set, &["eth0".into()]).unwrap();
    let backup_dir = fs.dir.join("backup");
    let manifest = adapter.backup(&plan, &backup_dir).unwrap();
    adapter.apply(&plan).unwrap();

    let tools = ToolPaths {
        ifup: Some(fake_ifup(&fs.dir, 2)),
    };
    let validation = adapter.validate(&file_set, &tools).unwrap();
    assert!(matches!(
        validation,
        Validation::Failed { exit: Some(2), .. }
    ));

    adapter.restore(&manifest).unwrap();
    assert_eq!(std::fs::read(&fs.interfaces).unwrap(), fs.original_main);
}

#[test]
fn guarded_transaction_restore_preserves_external_change() {
    let fs = rootfs("guarded-external-change");
    let adapter = IfupdownAdapter::new();
    let file_set = adapter
        .collect(&FileSources::new(fs.interfaces.clone()))
        .unwrap();
    let plan = adapter.plan_unmanage(&file_set, &["eth0".into()]).unwrap();
    let manifest = adapter.backup(&plan, &fs.dir.join("backup")).unwrap();

    std::fs::write(&fs.interfaces, b"changed by another process\n").unwrap();
    let error = adapter.restore_if_unchanged(&manifest, &plan).unwrap_err();
    assert!(matches!(error, HostNetError::ConcurrentModification { .. }));
    assert_eq!(
        std::fs::read(&fs.interfaces).unwrap(),
        b"changed by another process\n"
    );
}

#[test]
fn transactional_entry_does_not_overwrite_external_change_after_backup() {
    let fs = rootfs("transaction-external-change");
    let adapter = ExternalChangeAdapter {
        inner: IfupdownAdapter::new(),
        path: fs.interfaces.clone(),
    };
    let error = adapter
        .execute_unmanage(
            &FileSources::new(fs.interfaces.clone()),
            &["eth0".into()],
            &fs.dir.join("backup"),
            &ToolPaths::default(),
        )
        .unwrap_err();
    assert!(matches!(error, HostNetError::RecoveryFailed { .. }));
    assert_eq!(
        std::fs::read(&fs.interfaces).unwrap(),
        b"changed by another process\n"
    );
}

#[test]
fn unmanaged_interface_produces_empty_plan() {
    let fs = rootfs("unmanaged");
    let adapter = IfupdownAdapter::new();
    let file_set = adapter
        .collect(&FileSources::new(fs.interfaces.clone()))
        .unwrap();
    let plan = adapter.plan_unmanage(&file_set, &["eth9".into()]).unwrap();
    assert!(plan.edits.is_empty());
}

#[test]
fn validation_without_tools_is_unavailable() {
    let fs = rootfs("no-tools");
    let adapter = IfupdownAdapter::new();
    let file_set = adapter
        .collect(&FileSources::new(fs.interfaces.clone()))
        .unwrap();
    let validation = adapter.validate(&file_set, &ToolPaths::default()).unwrap();
    assert_eq!(validation, Validation::Unavailable);
}

#[test]
fn missing_interfaces_file_is_an_empty_operation() {
    let dir = temp_dir("no-interfaces");
    let adapter = IfupdownAdapter::new();
    let file_set = adapter
        .collect(&FileSources::new(dir.join("interfaces")))
        .unwrap();
    assert!(file_set.is_empty());
    let plan = adapter.plan_unmanage(&file_set, &["eth0".into()]).unwrap();
    assert!(plan.edits.is_empty());
    let backup_dir = dir.join("backup");
    let manifest = adapter.backup(&plan, &backup_dir).unwrap();
    assert!(manifest.files.is_empty());
    adapter.restore(&manifest).unwrap();
}

#[test]
fn ppp_wan_fails_preflight_with_unsupported_method() {
    let dir = temp_dir("ppp");
    let interfaces = dir.join("interfaces");
    std::fs::write(
        &interfaces,
        b"auto eth0\niface eth0 inet ppp\n    provider isp\n",
    )
    .unwrap();
    let adapter = IfupdownAdapter::new();
    let file_set = adapter
        .collect(&FileSources::new(interfaces.clone()))
        .unwrap();
    let error = adapter
        .plan_unmanage(&file_set, &["eth0".into()])
        .unwrap_err();
    assert!(matches!(error, HostNetError::UnsupportedMethod { .. }));
}

#[test]
fn transactional_entry_restores_after_validation_failure() {
    let fs = rootfs("transaction-failure");
    let adapter = IfupdownAdapter::new();
    let tools = ToolPaths {
        ifup: Some(fake_ifup(&fs.dir, 2)),
    };
    let error = adapter
        .execute_unmanage(
            &FileSources::new(fs.interfaces.clone()),
            &["eth0".into(), "eth1".into()],
            &fs.dir.join("backup"),
            &tools,
        )
        .unwrap_err();
    assert!(matches!(error, HostNetError::ValidationFailed { .. }));
    assert_eq!(std::fs::read(&fs.interfaces).unwrap(), fs.original_main);
    assert_eq!(std::fs::read(&fs.fragment).unwrap(), fs.original_fragment);
}

#[test]
fn atomic_rewrite_preserves_mode_under_restrictive_umask() {
    const CHILD_ENV: &str = "LKIT_HOSTNET_UMASK_CHILD";
    if std::env::var_os(CHILD_ENV).is_some() {
        // SAFETY: this branch runs in a dedicated child process before spawning threads.
        unsafe { libc::umask(0o077) };
        let fs = rootfs("restrictive-umask-child");
        std::fs::set_permissions(&fs.interfaces, std::fs::Permissions::from_mode(0o666)).unwrap();

        let adapter = IfupdownAdapter::new();
        let file_set = adapter
            .collect(&FileSources::new(fs.interfaces.clone()))
            .unwrap();
        let plan = adapter.plan_unmanage(&file_set, &["eth0".into()]).unwrap();
        adapter.apply(&plan).unwrap();

        let mode = std::fs::metadata(&fs.interfaces)
            .unwrap()
            .permissions()
            .mode()
            & 0o7777;
        assert_eq!(mode, 0o666);
        return;
    }

    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "atomic_rewrite_preserves_mode_under_restrictive_umask",
            "--nocapture",
        ])
        .env(CHILD_ENV, "1")
        .status()
        .unwrap();
    assert!(status.success());
}
