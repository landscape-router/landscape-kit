//! Debian 容器中的真实 ifupdown 兼容性测试。普通 cargo test 会跳过此测试。

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use lkit_hostnet::{
    FileSources, HostNetworkAdapter, ToolPaths, Validation, ifupdown::IfupdownAdapter,
};

fn temp_dir() -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("lkit-hostnet-real-ifupdown-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn required_tool(variable: &str) -> PathBuf {
    let path = std::env::var_os(variable)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("{variable} must name the real container tool"));
    assert!(path.is_file(), "{} does not exist", path.display());
    assert_ne!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o111,
        0,
        "{} is not executable",
        path.display()
    );
    path
}

fn assert_ifquery_accepts(ifquery: &Path, interfaces: &Path) {
    let output = Command::new(ifquery)
        .arg(format!("--interfaces={}", interfaces.display()))
        .arg("--list")
        .arg("--all")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "ifquery rejected {}: {}",
        interfaces.display(),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "requires Debian ifupdown; run through test-hostnet-ifupdown.yml"]
fn real_ifupdown_accepts_original_edit_and_restore() {
    let ifup = required_tool("LKIT_REAL_IFUP");
    let ifquery = required_tool("LKIT_REAL_IFQUERY");
    let dir = temp_dir();
    let fragments = dir.join("interfaces.d");
    std::fs::create_dir_all(&fragments).unwrap();
    let interfaces = dir.join("interfaces");
    let original_main = b"auto lkit0\niface ethernet inet static\nmtu 1400\niface lkit0 inet static inherits ethernet\naddress 192.0.2.2/\\\n24\ngateway 192.0.2.1\n  source-directory interfaces.d\n";
    let original_fragment = b"allow-custom lkit1\niface lkit1 inet dhcp\nhostname lkit-test\n";
    std::fs::write(&interfaces, original_main).unwrap();
    std::fs::write(fragments.join("lan"), original_fragment).unwrap();

    assert_ifquery_accepts(&ifquery, &interfaces);
    let adapter = IfupdownAdapter::new();
    let outcome = adapter
        .execute_unmanage(
            &FileSources::new(interfaces.clone()),
            &["lkit0".into(), "lkit1".into()],
            &dir.join("backup"),
            &ToolPaths { ifup: Some(ifup) },
        )
        .unwrap();
    assert_eq!(outcome.validation, Validation::Clean);
    assert_ifquery_accepts(&ifquery, &interfaces);

    let main = std::fs::read_to_string(&interfaces).unwrap();
    assert!(main.contains("iface lkit0 inet manual"));
    assert!(!main.contains("auto lkit0"));
    let fragment = std::fs::read_to_string(fragments.join("lan")).unwrap();
    assert_eq!(fragment, "iface lkit1 inet manual\n");

    adapter.restore(&outcome.manifest.unwrap()).unwrap();
    assert_eq!(std::fs::read(&interfaces).unwrap(), original_main);
    assert_eq!(
        std::fs::read(fragments.join("lan")).unwrap(),
        original_fragment
    );
    assert_ifquery_accepts(&ifquery, &interfaces);
    let _ = std::fs::remove_dir_all(dir);
}
