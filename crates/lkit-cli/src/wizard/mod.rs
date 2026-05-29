//! Wizard state machine — guided install flow with back-navigation.

pub mod nic_scan;
pub mod steps;

use std::net::Ipv4Addr;

use lkit_core::{
    InstallConfig, LanSetup, LandscapeServiceConfig, NetworkSetup, SourceSelection, WanMode,
    WanSetup,
};

/// Wizard step identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepKind {
    /// Step 1: Select WAN NIC.
    WanSelect,
    /// Step 2: Select LAN NICs (skipped in single-NIC mode).
    LanSelect,
    /// Step 3: WAN IP mode.
    WanConfig,
    /// Step 4: LAN gateway config (skipped in single-NIC mode).
    LanConfig,
    /// Step 5: Landscape service settings.
    LandscapeService,
    /// Step 6: Install source and version.
    Source,
    /// Step 7: Summary and confirmation.
    Summary,
}

/// Navigation action returned by each step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WizardAction {
    /// Proceed to the next step.
    Next,
    /// Go back to the previous step.
    Back,
    /// Abort the wizard.
    Quit,
}

/// Definition of a wizard step.
#[derive(Debug)]
pub struct StepDef {
    /// Which step this is.
    pub kind: StepKind,
    /// Short title shown in the header.
    pub title: &'static str,
    /// Help text shown to the user.
    pub help_text: &'static str,
}

/// Intermediate data collected across all wizard steps.
#[derive(Debug, Default, Clone)]
pub struct CollectedConfig {
    /// Step 1: selected WAN NIC name.
    pub wan_nic: Option<String>,
    /// Step 2: selected LAN NIC names.
    pub lan_nics: Vec<String>,
    /// Step 3: WAN IP mode.
    pub wan_mode: Option<WanMode>,
    /// Step 4: LAN gateway IP.
    pub lan_gateway: Option<Ipv4Addr>,
    /// Step 4: LAN subnet mask.
    pub lan_mask: Option<u8>,
    /// Step 5: web UI port (default 6300).
    pub web_port: Option<u16>,
    /// Step 5: admin username (default "root").
    pub admin_user: Option<String>,
    /// Step 5: admin password.
    pub admin_pass: Option<String>,
    /// Step 6: source name (None = auto-detect).
    pub source_name: Option<String>,
    /// Step 6: version tag (None = latest).
    pub version: Option<String>,
}

/// The wizard state machine.
///
/// Collects network and service configuration through a series of interactive steps.
/// Supports back-navigation with cascading field clearing.
pub struct Wizard {
    steps: Vec<StepDef>,
    current: usize,
    /// Whether we have only one NIC (skips LAN steps).
    single_nic: bool,
    /// Intermediate collected data.
    pub collected: CollectedConfig,
}

impl Wizard {
    /// Create a new wizard with the default step sequence.
    ///
    /// `single_nic` — when true, LAN-related steps (LanSelect, LanConfig) are skipped.
    pub fn new(single_nic: bool) -> Self {
        let steps = vec![
            StepDef {
                kind: StepKind::WanSelect,
                title: "WAN 网卡选择",
                help_text: "WAN 是路由器连接外网的接口。选择连接光猫或上级路由器的那根网线对应的网卡。",
            },
            StepDef {
                kind: StepKind::LanSelect,
                title: "LAN 网卡选择",
                help_text: "LAN 是路由器向内网设备提供服务的接口。选中的网卡会合并为一个 bridge（br_lan）。",
            },
            StepDef {
                kind: StepKind::WanConfig,
                title: "WAN 接入方式",
                help_text: "DHCP：自动获取 IP。静态 IP：手动指定。不配置：稍后在 Web UI 设置。",
            },
            StepDef {
                kind: StepKind::LanConfig,
                title: "LAN 网关配置",
                help_text: "设置 br_lan 的 IP 地址，作为内网设备的网关。",
            },
            StepDef {
                kind: StepKind::LandscapeService,
                title: "Landscape 服务配置",
                help_text: "配置 Web 管理界面的端口和管理员账号。",
            },
            StepDef {
                kind: StepKind::Source,
                title: "安装源与版本",
                help_text: "选择 Landscape 的下载源和版本。",
            },
            StepDef {
                kind: StepKind::Summary,
                title: "确认安装",
                help_text: "检查配置摘要，确认后开始安装。",
            },
        ];

        Self {
            steps,
            current: 0,
            single_nic,
            collected: CollectedConfig::default(),
        }
    }

    /// Current step definition.
    pub fn current_step(&self) -> &StepDef {
        &self.steps[self.current]
    }

    /// Current step index (0-based).
    pub fn current_index(&self) -> usize {
        self.current
    }

    /// Total number of registered steps (including skipped ones).
    #[allow(dead_code)]
    pub fn total_steps(&self) -> usize {
        self.steps.len()
    }

    /// Whether the current step should be skipped.
    pub fn should_skip_current(&self) -> bool {
        if self.single_nic {
            matches!(
                self.current_step().kind,
                StepKind::LanSelect | StepKind::LanConfig
            )
        } else {
            false
        }
    }

    /// Advance to the next step.
    ///
    /// Skips LAN steps when in single-NIC mode.
    /// Returns `true` if there is a next step, `false` if we've reached the end.
    pub fn advance(&mut self) -> bool {
        self.current += 1;
        // Skip LAN steps in single-NIC mode
        while self.current < self.steps.len() && self.should_skip_current() {
            self.current += 1;
        }
        self.current < self.steps.len()
    }

    /// Retreat to the previous step with cascading field clearing.
    ///
    /// - Going back from WanConfig to WanSelect clears `wan_mode`.
    /// - Going back from LanConfig to LanSelect preserves `lan_gateway` / `lan_mask`.
    ///
    /// Returns `true` if there is a previous step, `false` if already at the first step.
    pub fn retreat(&mut self) -> bool {
        if self.current == 0 {
            return false;
        }

        // Move back, skipping LAN steps in single-NIC mode
        self.current -= 1;
        while self.current > 0 && self.should_skip_current() {
            self.current -= 1;
        }

        // Cascading clear rules
        let target_kind = self.current_step().kind;

        // Clear wan_mode when retreating to WanSelect — the WAN NIC may change.
        // Handles both single-NIC (WanConfig→WanSelect) and multi-NIC
        // (WanConfig→LanSelect→WanSelect) paths.
        if target_kind == StepKind::WanSelect {
            self.collected.wan_mode = None;
        }
        // lan_gateway / lan_mask are preserved when retreating from LanConfig
        // to LanSelect — the gateway is on br_lan, independent of member NICs.

        true
    }

    /// Build an [`InstallConfig`] from the collected data.
    ///
    /// Returns `Err` if required fields are missing.
    pub fn build_config(&self, landscape_version: String) -> Result<InstallConfig, String> {
        let wan_nic = self
            .collected
            .wan_nic
            .as_ref()
            .ok_or("WAN NIC not selected")?;

        let wan_mode = self
            .collected
            .wan_mode
            .clone()
            .ok_or("WAN mode not selected")?;

        let lan = if self.collected.lan_nics.is_empty() {
            None
        } else {
            let gateway = self
                .collected
                .lan_gateway
                .ok_or("LAN gateway not configured")?;
            let mask = self.collected.lan_mask.ok_or("LAN mask not configured")?;
            Some(LanSetup {
                member_nics: self.collected.lan_nics.clone(),
                gateway,
                mask,
            })
        };

        let web_port = self.collected.web_port.unwrap_or(6300);
        let admin_user = self
            .collected
            .admin_user
            .clone()
            .unwrap_or_else(|| "root".to_string());
        let admin_pass = self
            .collected
            .admin_pass
            .as_ref()
            .ok_or("admin password not set")?
            .clone();

        Ok(InstallConfig {
            network: NetworkSetup {
                wan: WanSetup {
                    iface_name: wan_nic.clone(),
                    mode: wan_mode,
                },
                lan,
            },
            landscape: LandscapeServiceConfig {
                web_port,
                admin_user,
                admin_pass,
            },
            source: SourceSelection {
                source_name: self.collected.source_name.clone(),
                version: self.collected.version.clone(),
            },
            landscape_version,
        })
    }

    /// Run the wizard interactively.
    ///
    /// Renders each step in sequence, handling back-navigation and quit.
    /// Returns `Ok(Some(config))` on completion, `Ok(None)` if the user quit.
    pub fn run(&mut self, nics: &[nic_scan::NicInfo]) -> anyhow::Result<Option<InstallConfig>> {
        loop {
            // Skip LAN steps if single-NIC
            if self.should_skip_current() {
                if !self.advance() {
                    break;
                }
                continue;
            }

            let kind = self.current_step().kind;
            // Print step header
            let step_num = self.current_index() + 1;
            let title = self.current_step().title;
            let help = self.current_step().help_text;
            eprintln!();
            eprintln!("  [{step_num}/7] {title}");
            eprintln!("  {help}");
            eprintln!();

            match steps::render_step(kind, &mut self.collected, nics)? {
                WizardAction::Next => {
                    if !self.advance() {
                        break;
                    }
                }
                WizardAction::Back => {
                    if !self.retreat() {
                        return Ok(None);
                    }
                }
                WizardAction::Quit => return Ok(None),
            }
        }

        // Build config from collected data
        // landscape_version will be resolved from release manifest later
        let version = self
            .collected
            .version
            .clone()
            .unwrap_or_else(|| "latest".to_string());
        match self.build_config(version) {
            Ok(config) => Ok(Some(config)),
            Err(e) => Err(anyhow::anyhow!("配置不完整: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Walk forward through all steps.
    #[test]
    fn test_advance_forward() {
        let mut w = Wizard::new(false);
        assert_eq!(w.current_index(), 0);

        // Step 0 → 1 (LanSelect)
        assert!(w.advance());
        assert_eq!(w.current_step().kind, StepKind::LanSelect);

        // Step 1 → 2 (WanConfig)
        assert!(w.advance());
        assert_eq!(w.current_step().kind, StepKind::WanConfig);

        // Step 2 → 3 (LanConfig)
        assert!(w.advance());
        assert_eq!(w.current_step().kind, StepKind::LanConfig);

        // Step 3 → 4 (LandscapeService)
        assert!(w.advance());
        assert_eq!(w.current_step().kind, StepKind::LandscapeService);

        // Step 4 → 5 (Source)
        assert!(w.advance());
        assert_eq!(w.current_step().kind, StepKind::Source);

        // Step 5 → 6 (Summary)
        assert!(w.advance());
        assert_eq!(w.current_step().kind, StepKind::Summary);

        // Step 6 → past end
        assert!(!w.advance());
    }

    /// Retreating from WanConfig to WanSelect clears wan_mode.
    ///
    /// In single-NIC mode, WanConfig retreats directly to WanSelect (skipping LanSelect).
    /// In multi-NIC mode, WanConfig retreats to LanSelect first — wan_mode is NOT cleared
    /// because LAN changes don't affect WAN. The clear happens only when reaching WanSelect.
    #[test]
    fn test_retreat_clears_wan_mode() {
        // Single-NIC: WanConfig → WanSelect directly, clears wan_mode
        let mut w = Wizard::new(true);
        w.collected.wan_mode = Some(WanMode::Dhcp);

        assert!(w.advance());
        assert_eq!(w.current_step().kind, StepKind::WanConfig);

        assert!(w.retreat());
        assert_eq!(w.current_step().kind, StepKind::WanSelect);
        assert!(
            w.collected.wan_mode.is_none(),
            "wan_mode should be cleared when retreating to WanSelect"
        );

        // Multi-NIC: WanConfig → LanSelect, wan_mode preserved
        let mut w2 = Wizard::new(false);
        w2.collected.wan_mode = Some(WanMode::Dhcp);

        w2.advance(); // → LanSelect
        w2.advance(); // → WanConfig
        assert_eq!(w2.current_step().kind, StepKind::WanConfig);

        w2.retreat(); // → LanSelect (wan_mode preserved — LAN changes don't affect WAN)
        assert_eq!(w2.current_step().kind, StepKind::LanSelect);
        assert!(
            w2.collected.wan_mode.is_some(),
            "wan_mode preserved on WanConfig→LanSelect retreat"
        );

        w2.retreat(); // → WanSelect (now clear)
        assert_eq!(w2.current_step().kind, StepKind::WanSelect);
        assert!(
            w2.collected.wan_mode.is_none(),
            "wan_mode cleared when reaching WanSelect"
        );
    }

    /// Retreating from LanConfig to LanSelect preserves lan_gateway.
    #[test]
    fn test_retreat_preserves_lan_gateway() {
        let mut w = Wizard::new(false);
        let gw = Ipv4Addr::new(192, 168, 5, 1);
        w.collected.lan_gateway = Some(gw);
        w.collected.lan_mask = Some(24);

        // Navigate to LanConfig (step 3): 0→1→2→3
        w.advance(); // LanSelect
        w.advance(); // WanConfig
        w.advance(); // LanConfig
        assert_eq!(w.current_step().kind, StepKind::LanConfig);

        // Retreat to LanSelect (step 1): LanConfig → WanConfig → LanSelect
        assert!(w.retreat()); // → WanConfig
        assert!(w.retreat()); // → LanSelect
        assert_eq!(w.current_step().kind, StepKind::LanSelect);
        assert_eq!(w.collected.lan_gateway, Some(gw));
        assert_eq!(w.collected.lan_mask, Some(24));
    }

    /// Single-NIC mode skips LanSelect and LanConfig.
    #[test]
    fn test_single_nic_skips_lan_steps() {
        let mut w = Wizard::new(true);
        assert_eq!(w.current_step().kind, StepKind::WanSelect);

        // Should skip LanSelect → go directly to WanConfig
        assert!(w.advance());
        assert_eq!(w.current_step().kind, StepKind::WanConfig);

        // Should skip LanConfig → go directly to LandscapeService
        assert!(w.advance());
        assert_eq!(w.current_step().kind, StepKind::LandscapeService);

        assert!(w.advance());
        assert_eq!(w.current_step().kind, StepKind::Source);

        assert!(w.advance());
        assert_eq!(w.current_step().kind, StepKind::Summary);

        assert!(!w.advance());
    }

    /// Quit returns None equivalent (tested via build_config failure).
    #[test]
    fn test_quit_returns_none() {
        let w = Wizard::new(false);
        // At step 0, if user quits, no config is built.
        let result = w.build_config("0.19.2".to_string());
        assert!(result.is_err(), "should fail with missing fields");
    }

    /// build_config succeeds with complete data.
    #[test]
    fn test_build_config_success() -> Result<(), Box<dyn std::error::Error>> {
        let mut w = Wizard::new(false);
        w.collected.wan_nic = Some("eth0".to_string());
        w.collected.lan_nics = vec!["eth1".to_string()];
        w.collected.wan_mode = Some(WanMode::Dhcp);
        w.collected.lan_gateway = Some(Ipv4Addr::new(192, 168, 5, 1));
        w.collected.lan_mask = Some(24);
        w.collected.web_port = Some(6300);
        w.collected.admin_user = Some("root".to_string());
        w.collected.admin_pass = Some("secret".to_string());

        let config = w.build_config("0.19.2".to_string())?;
        assert_eq!(config.network.wan.iface_name, "eth0");
        assert!(config.network.lan.is_some());
        assert_eq!(config.landscape_version, "0.19.2");
        Ok(())
    }
}
