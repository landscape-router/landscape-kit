use super::*;

struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("lkit-openrc-check-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(root.join("init.d")).unwrap();
        std::fs::create_dir_all(root.join("runlevels/default")).unwrap();
        Self(root)
    }
    fn service(&self, name: &str) {
        std::fs::write(self.0.join("init.d").join(name), "fixture").unwrap();
    }
    fn info(&self, names: &[&str], query: impl Fn(&str) -> Option<i32>) -> Option<UnitInfo> {
        openrc_info_at(
            &self.0.join("init.d"),
            &self.0.join("runlevels"),
            names,
            query,
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn missing_openrc_services_are_not_unknown_and_do_not_query_systemctl() {
    let fixture = Fixture::new();
    let info = fixture
        .info(
            &[
                "networkmanager",
                "NetworkManager",
                "systemd-resolved",
                "firewalld",
            ],
            |_| panic!("absent service must not be queried"),
        )
        .unwrap();
    assert_eq!(
        info,
        UnitInfo {
            installed: false,
            active: false,
            enabled: false
        }
    );
}

#[test]
fn openrc_aliases_and_runlevels_are_checked_without_real_service_processes() {
    let fixture = Fixture::new();
    fixture.service("NetworkManager");
    std::os::unix::fs::symlink(
        fixture.0.join("init.d/NetworkManager"),
        fixture.0.join("runlevels/default/NetworkManager"),
    )
    .unwrap();
    let info = fixture
        .info(&["networkmanager", "NetworkManager"], |name| {
            assert_eq!(name, "NetworkManager");
            Some(0)
        })
        .unwrap();
    assert_eq!(
        info,
        UnitInfo {
            installed: true,
            active: true,
            enabled: true
        }
    );
}

#[test]
fn stopped_openrc_services_are_distinct_from_query_failures() {
    let fixture = Fixture::new();
    fixture.service("firewalld");
    assert_eq!(
        fixture.info(&["firewalld"], |_| Some(3)).unwrap(),
        UnitInfo {
            installed: true,
            active: false,
            enabled: false
        }
    );
    assert!(fixture.info(&["firewalld"], |_| None).is_none());
    assert!(fixture.info(&["firewalld"], |_| Some(32)).is_none());
}
