use super::*;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "lkit-alpine-base-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
    }

    fn probe(&self, name: &str) -> Option<PathBuf> {
        let candidate = self.0.join(name);
        candidate.is_file().then_some(candidate)
    }

    fn installed(&self, package: BasePackage) -> bool {
        package.installed_with(
            Some(PackageManager::Apk),
            |name| self.probe(name),
            &self.0.join("plugins"),
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn alpine_ppp_requires_both_pppd_and_a_pppoe_plugin() {
    let fixture = Fixture::new();
    assert!(!fixture.installed(BasePackage::Ppp));
    std::fs::write(fixture.0.join("pppd"), "fixture").unwrap();
    assert!(!fixture.installed(BasePackage::Ppp));
    let version = fixture.0.join("plugins/2.5.2");
    std::fs::create_dir_all(&version).unwrap();
    std::fs::write(version.join("pppoe.so"), "fixture").unwrap();
    assert!(fixture.installed(BasePackage::Ppp));
    std::fs::remove_file(version.join("pppoe.so")).unwrap();
    assert!(!fixture.installed(BasePackage::Ppp));
    std::fs::write(version.join("rp-pppoe.so"), "fixture").unwrap();
    assert!(fixture.installed(BasePackage::Ppp));
    std::fs::remove_file(fixture.0.join("pppd")).unwrap();
    assert!(!fixture.installed(BasePackage::Ppp));
}

#[test]
fn non_apk_detection_keeps_the_binary_only_contract() {
    let fixture = Fixture::new();
    std::fs::write(fixture.0.join("pppd"), "fixture").unwrap();
    assert!(BasePackage::Ppp.installed_with(
        Some(PackageManager::Apt),
        |name| fixture.probe(name),
        &fixture.0.join("missing-plugins"),
    ));
}

#[test]
fn alpine_requires_full_iproute2_and_procps_not_busybox_applets() {
    let fixture = Fixture::new();
    let busybox = fixture.0.join("busybox");
    std::fs::write(&busybox, "fixture").unwrap();
    for name in ["ip", "sysctl"] {
        std::os::unix::fs::symlink(&busybox, fixture.0.join(name)).unwrap();
    }
    assert!(!fixture.installed(BasePackage::Iproute2));
    assert!(!fixture.installed(BasePackage::Procps));
    std::fs::write(fixture.0.join("tc"), "fixture").unwrap();
    assert!(!fixture.installed(BasePackage::Iproute2));
    for name in ["ip", "sysctl"] {
        std::fs::remove_file(fixture.0.join(name)).unwrap();
        std::fs::write(fixture.0.join(name), "fixture").unwrap();
    }
    assert!(fixture.installed(BasePackage::Iproute2));
    assert!(fixture.installed(BasePackage::Procps));
    std::fs::remove_file(fixture.0.join("tc")).unwrap();
    assert!(!fixture.installed(BasePackage::Iproute2));
    std::os::unix::fs::symlink(&busybox, fixture.0.join("tc")).unwrap();
    assert!(!fixture.installed(BasePackage::Iproute2));
}

#[test]
fn alpine_install_plan_includes_the_pppoe_subpackage_and_correct_package_names() {
    assert_eq!(
        install_args(&BasePackage::all(), PackageManager::Apk),
        vec![
            "add",
            "--no-cache",
            "ppp",
            "ppp-pppoe",
            "iproute2",
            "iw",
            "hostapd",
            "procps-ng",
        ]
    );
    assert_eq!(
        install_args(&[BasePackage::Ppp], PackageManager::Apt),
        vec!["install", "-y", "ppp"]
    );
}
