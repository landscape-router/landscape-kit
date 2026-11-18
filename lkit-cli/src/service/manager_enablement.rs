use super::{Registration, RegistrationKind, ServiceBefore, ServiceManagerKind};
use crate::deployment::plan::InstallError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeSet;
use std::path::{Component, Path};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SystemdStartupLinks {
    pub version: u32,
    pub boot_id: String,
    pub persistent: Vec<String>,
    pub runtime: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Enablement {
    pub enabled: bool,
    pub systemd_links: Option<SystemdStartupLinks>,
}

fn invalid() -> InstallError {
    InstallError::CorruptedTransaction("invalid service enablement snapshot".into())
}

pub(crate) fn validate_membership(name: &str) -> Result<(), InstallError> {
    let unit = name
        .strip_suffix(".wants")
        .or_else(|| name.strip_suffix(".requires"))
        .ok_or_else(invalid)?;
    let mut components = Path::new(unit).components();
    if !matches!(components.next(), Some(Component::Normal(_)))
        || components.next().is_some()
        || name.chars().any(char::is_control)
        || name.contains('/')
    {
        return Err(invalid());
    }
    Ok(())
}

impl SystemdStartupLinks {
    pub(crate) fn validate(&self) -> Result<(), InstallError> {
        if self.version != 1 || uuid::Uuid::parse_str(&self.boot_id).is_err() {
            return Err(invalid());
        }
        for members in [&self.persistent, &self.runtime] {
            let mut seen = BTreeSet::new();
            for name in members {
                validate_membership(name)?;
                if !seen.insert(name) {
                    return Err(invalid());
                }
            }
        }
        Ok(())
    }
}

impl Enablement {
    pub(crate) fn validate(&self) -> Result<(), InstallError> {
        if let Some(links) = &self.systemd_links {
            links.validate()?;
            if self.enabled != (!links.persistent.is_empty() || !links.runtime.is_empty()) {
                return Err(invalid());
            }
        }
        Ok(())
    }
    pub(crate) fn enabled_for_boot(&self, boot: &str) -> bool {
        self.systemd_links.as_ref().map_or(self.enabled, |links| {
            !links.persistent.is_empty() || (links.boot_id == boot && !links.runtime.is_empty())
        })
    }
    pub(crate) fn enabled_now(&self) -> Result<bool, InstallError> {
        self.validate()?;
        if self.systemd_links.is_none() {
            return Ok(self.enabled);
        }
        Ok(self.enabled_for_boot(&crate::network::takeover::current_boot_id()?))
    }
    pub(crate) fn matches_current(&self, current: &Self) -> Result<bool, InstallError> {
        self.validate()?;
        current.validate()?;
        let Some(expected) = &self.systemd_links else {
            return Ok(self.enabled == current.enabled);
        };
        let Some(actual) = &current.systemd_links else {
            return Ok(false);
        };
        let persistent: BTreeSet<_> = expected.persistent.iter().collect();
        let runtime: BTreeSet<_> = expected
            .runtime
            .iter()
            .filter(|_| expected.boot_id == actual.boot_id)
            .collect();
        Ok(self.enabled_for_boot(&actual.boot_id) == current.enabled
            && persistent == actual.persistent.iter().collect()
            && runtime == actual.runtime.iter().collect())
    }
}

impl ServiceBefore {
    pub(crate) fn validate_for_backend(
        &self,
        backend: ServiceManagerKind,
    ) -> Result<(), InstallError> {
        self.validate()?;
        if self.systemd_links.is_some() && backend != ServiceManagerKind::Systemd {
            return Err(invalid());
        }
        Ok(())
    }
    pub(crate) fn enablement(&self) -> Enablement {
        Enablement {
            enabled: self.enabled,
            systemd_links: self.systemd_links.clone(),
        }
    }
    pub(crate) fn enabled_now(&self) -> Result<bool, InstallError> {
        self.enablement().enabled_now()
    }
    pub(crate) fn matches_current_state(
        &self,
        current: &Enablement,
        active: bool,
    ) -> Result<bool, InstallError> {
        let same_boot = self
            .systemd_links
            .as_ref()
            .zip(current.systemd_links.as_ref())
            .is_none_or(|(before, now)| before.boot_id == now.boot_id);
        // Active state is a fact about one kernel boot. A definition-only repair
        // never starts/stops the service to replay activity from an expired boot.
        Ok(self.enablement().matches_current(current)? && (!same_boot || self.active == active))
    }
    pub(crate) fn validate(&self) -> Result<(), InstallError> {
        self.enablement().validate()?;
        if self.systemd_links.is_some()
            && self.registration.kind == RegistrationKind::Missing
            && self.enabled
        {
            return Err(invalid());
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExactEnabled {
    version: u32,
    enabled: bool,
    systemd_links: SystemdStartupLinks,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum EnabledWire {
    Legacy(bool),
    Exact(ExactEnabled),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    registration: Registration,
    enabled: EnabledWire,
    active: bool,
}

impl Serialize for ServiceBefore {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.validate().map_err(serde::ser::Error::custom)?;
        let enabled = match &self.systemd_links {
            Some(links) => EnabledWire::Exact(ExactEnabled {
                version: 1,
                enabled: self.enabled,
                systemd_links: links.clone(),
            }),
            None => EnabledWire::Legacy(self.enabled),
        };
        Wire {
            registration: self.registration.clone(),
            enabled,
            active: self.active,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ServiceBefore {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = Wire::deserialize(deserializer)?;
        let (enabled, systemd_links) = match wire.enabled {
            EnabledWire::Legacy(enabled) => (enabled, None),
            EnabledWire::Exact(exact) => {
                if exact.version != 1 {
                    return Err(serde::de::Error::custom(
                        "unsupported service enablement version",
                    ));
                }
                (exact.enabled, Some(exact.systemd_links))
            }
        };
        let before = Self {
            registration: wire.registration,
            enabled,
            active: wire.active,
            systemd_links,
        };
        before.validate().map_err(serde::de::Error::custom)?;
        Ok(before)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const BOOT: &str = "11111111-1111-4111-8111-111111111111";
    const NEXT: &str = "22222222-2222-4222-8222-222222222222";
    fn before() -> ServiceBefore {
        ServiceBefore {
            registration: Registration {
                kind: RegistrationKind::Symlink,
                target: Some("/install/service/landscape-router.service".into()),
            },
            enabled: true,
            active: true,
            systemd_links: Some(SystemdStartupLinks {
                version: 1,
                boot_id: BOOT.into(),
                persistent: vec![],
                runtime: vec!["multi-user.target.wants".into()],
            }),
        }
    }
    #[test]
    fn exact_snapshot_roundtrips_and_old_bool_readers_reject_it() {
        let before = before();
        let value = serde_json::to_value(&before).unwrap();
        assert!(value["enabled"].is_object());
        assert!(serde_json::from_value::<bool>(value["enabled"].clone()).is_err());
        assert_eq!(
            serde_json::from_value::<ServiceBefore>(value).unwrap(),
            before
        );
        let mut legacy = before.clone();
        legacy.systemd_links = None;
        let value = serde_json::to_value(&legacy).unwrap();
        assert_eq!(value["enabled"], true);
        assert_eq!(
            serde_json::from_value::<ServiceBefore>(value).unwrap(),
            legacy
        );
    }
    #[test]
    fn runtime_enablement_expires_only_across_host_boots() {
        let mut exact = before().enablement();
        assert!(exact.enabled_for_boot(BOOT));
        assert!(!exact.enabled_for_boot(NEXT));
        exact
            .systemd_links
            .as_mut()
            .unwrap()
            .persistent
            .push("custom.target.requires".into());
        assert!(exact.enabled_for_boot(NEXT));
        let legacy = Enablement {
            enabled: true,
            systemd_links: None,
        };
        assert!(legacy.enabled_for_boot(NEXT));
    }
    #[test]
    fn state_verification_preserves_same_boot_activity_without_replaying_expired_activity() {
        let before = before();
        let current = before.enablement();
        assert!(before.matches_current_state(&current, true).unwrap());
        assert!(!before.matches_current_state(&current, false).unwrap());
        let mut rebooted = current.clone();
        let links = rebooted.systemd_links.as_mut().unwrap();
        links.boot_id = NEXT.into();
        links.runtime.clear();
        rebooted.enabled = false;
        assert!(before.matches_current_state(&rebooted, false).unwrap());
        assert!(before.matches_current_state(&rebooted, true).unwrap());
        let mut legacy = before;
        legacy.systemd_links = None;
        assert!(!legacy.matches_current_state(&current, false).unwrap());
    }
    #[test]
    fn enabled_boolean_does_not_hide_runtime_promoted_to_persistent() {
        let before = before().enablement();
        let mut current = before.clone();
        let links = current.systemd_links.as_mut().unwrap();
        links.persistent = std::mem::take(&mut links.runtime);
        assert!(before.enabled && current.enabled);
        assert!(!before.matches_current(&current).unwrap());
    }
    #[test]
    fn malformed_or_unknown_facts_are_rejected() {
        for field in ["version", "boot_id", "runtime"] {
            let mut value = serde_json::to_value(before()).unwrap();
            value["enabled"]["systemd_links"][field] = match field {
                "version" => 99.into(),
                "boot_id" => "unknown".into(),
                _ => serde_json::json!(["../outside.wants"]),
            };
            assert!(serde_json::from_value::<ServiceBefore>(value).is_err());
        }
        let mut value = serde_json::to_value(before()).unwrap();
        value["enabled"]["enabled"] = false.into();
        assert!(serde_json::from_value::<ServiceBefore>(value).is_err());
    }
}
