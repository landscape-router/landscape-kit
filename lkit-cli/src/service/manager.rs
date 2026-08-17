//! 服务管理器抽象:跨发行版 init 系统的中性契约。
//!
//! 契约只暴露 lkit 对服务的操作需求,不绑定任何具体 init 系统的概念
//! (systemd 的 unit 名、MainPID、daemon-reload、mask 等细节由各后端内部处理)。
//! 当前实现后端为 [`crate::service::systemd::Systemd`];OpenRC、runit、sysvinit
//! 等后端按需接入,接入时以真实操作驱动契约演进,不做投机抽象。

use std::any::Any;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::deployment::plan::InstallError;

/// 服务管理器后端标识。序列化在安装状态与事务文件中,
/// 新增后端时在此增加变体并处理状态 schema 演进。
///
/// lkit 明确依赖发行版自启服务:`none`(不托管运行态)不再是受支持的部署类型,
/// 安装时必须探测到可用的服务管理器,否则失败。
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ServiceManagerKind {
    Systemd,
    Openrc,
    Sysvinit,
}

impl ServiceManagerKind {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Systemd => "systemd",
            Self::Openrc => "openrc",
            Self::Sysvinit => "sysvinit",
        }
    }

    /// 当前受支持的后端集合,用于状态校验与探测顺序。
    pub(crate) fn supported() -> [Self; 3] {
        [Self::Systemd, Self::Openrc, Self::Sysvinit]
    }
}

/// lkit 需要托管的服务身份。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ManagedService {
    /// 当前 Landscape 路由器后端。
    LandscapeRouter,
    /// lkit 自身的常驻服务。
    LkitDaemon,
}

impl ManagedService {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::LandscapeRouter => "landscape-router",
            Self::LkitDaemon => "lkit",
        }
    }
}

/// 服务管理器可用性。
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Availability {
    /// 满足全部可用性条件。
    Available { version: String },
    /// 主机没有运行该 init 系统。
    NotDetected,
    /// 看似使用该 init 系统但环境损坏,或工具缺失/不可连接。
    Unavailable(String),
}

/// 系统注册路径的实时状态。
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SystemRegistration {
    /// 不存在注册链接。
    Missing,
    /// 指向受管定义原件的符号链接,记录原始目标。
    Symlink { target: PathBuf },
    /// 普通文件或其他无法证明归属的内容,属于所有权冲突。
    Conflict { file_type: String },
}

/// 序列化的事务前注册状态(service before JSON 的一部分)。
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct Registration {
    pub kind: RegistrationKind,
    pub target: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum RegistrationKind {
    Missing,
    Symlink,
}

/// 受管服务事务前状态,用于失败回滚恢复注册与 enabled/active 事实。
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct ServiceBefore {
    pub registration: Registration,
    pub enabled: bool,
    pub active: bool,
}

/// 服务管理器抽象。
pub(crate) trait ServiceManager: Send + Sync {
    fn kind(&self) -> ServiceManagerKind;

    /// 探测当前主机上的可用性。
    fn probe(&self) -> Availability;

    /// init 系统视角的服务名称(如 systemd unit 名)。
    fn service_name(&self, service: ManagedService) -> &str;

    /// 渲染受管服务定义内容。`canonical_root` 为真实安装根目录。
    fn render_definition(
        &self,
        service: ManagedService,
        canonical_root: &Path,
    ) -> Result<String, InstallError>;

    /// 校验受管服务定义仍满足安全不变量。
    fn validate_definition(
        &self,
        service: ManagedService,
        content: &str,
        canonical_root: &Path,
    ) -> Result<(), InstallError>;

    /// 查询系统注册路径的状态。
    fn query_registration(
        &self,
        service: ManagedService,
    ) -> Result<SystemRegistration, InstallError>;

    /// 创建系统注册链接(指向受管定义原件)。
    fn register(&self, service: ManagedService, origin: &Path) -> Result<(), InstallError>;

    /// 移除系统注册链接。
    fn unregister(&self, service: ManagedService, origin: &Path) -> Result<(), InstallError>;

    fn is_enabled(&self, service: ManagedService) -> Result<bool, InstallError>;
    fn enable(&self, service: ManagedService) -> Result<(), InstallError>;
    fn disable(&self, service: ManagedService) -> Result<(), InstallError>;
    fn is_active(&self, service: ManagedService) -> Result<bool, InstallError>;
    fn active_state(&self, service: ManagedService) -> Result<String, InstallError>;
    fn start(&self, service: ManagedService) -> Result<(), InstallError>;
    fn stop(&self, service: ManagedService) -> Result<(), InstallError>;
    fn restart(&self, service: ManagedService) -> Result<(), InstallError>;

    /// 停止服务并等待 `wait_for_exit` 判定进程退出,超时报错。
    fn stop_and_wait(
        &self,
        service: ManagedService,
        wait_for_exit: &dyn Fn() -> bool,
    ) -> Result<(), InstallError> {
        self.stop(service)?;
        for _ in 0..30 {
            if wait_for_exit() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        Err(InstallError::Systemd(
            "service did not exit within the timeout after stop".into(),
        ))
    }

    /// 重新加载服务定义(如 systemd daemon-reload);无此概念的后端为 no-op。
    fn refresh(&self) -> Result<(), InstallError> {
        Ok(())
    }

    fn main_pid(&self, service: ManagedService) -> Result<u32, InstallError>;

    /// 恢复注册与 enabled/active 状态(失败回滚用)。默认实现:
    /// 先恢复注册,再按事务前 active 事实启动或停止。
    fn restore_before(
        &self,
        service: ManagedService,
        before: &ServiceBefore,
        origin: &Path,
    ) -> Result<(), InstallError> {
        self.restore_registration(service, before, origin)?;
        if before.active {
            self.start(service)?;
        } else if self.is_active(service).unwrap_or(false) {
            self.stop(service)?;
        }
        Ok(())
    }

    /// 只恢复注册与 enabled 状态,不改变 active 状态(回滚顺序要求先恢复注册)。
    fn restore_registration(
        &self,
        service: ManagedService,
        before: &ServiceBefore,
        origin: &Path,
    ) -> Result<(), InstallError>;

    /// 主机 `/etc/resolv.conf` 路径(宿主状态,由管理器配置携带)。
    fn resolv_conf(&self) -> &Path;

    /// 向下转型,供需要具体后端能力的调用方(如 network takeover)使用。
    fn as_any(&self) -> &dyn Any;
}

/// 通用事务前状态捕获:注册 + enabled + active。
/// 注册所有权冲突时阻断(不能自动接管)。
pub(crate) fn capture_before(
    manager: &dyn ServiceManager,
    service: ManagedService,
) -> Result<ServiceBefore, InstallError> {
    let (kind, target) = match manager.query_registration(service)? {
        SystemRegistration::Missing => (RegistrationKind::Missing, None),
        SystemRegistration::Symlink { target } => (
            RegistrationKind::Symlink,
            Some(target.display().to_string()),
        ),
        SystemRegistration::Conflict { file_type } => {
            return Err(InstallError::Systemd(format!(
                "cannot take over {}: {file_type} ownership conflict",
                manager.service_name(service)
            )));
        }
    };
    Ok(ServiceBefore {
        registration: Registration { kind, target },
        enabled: manager.is_enabled(service)?,
        active: manager.is_active(service)?,
    })
}

/// 扫描 `/proc/*/cmdline`,返回第一个命令行包含 `pattern` 的进程 pid。
/// 供没有 MainPID 概念的 init 后端(OpenRC、sysvinit)解析主进程。
pub(crate) fn pid_of_command(pattern: &str) -> Option<i64> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return None;
    };
    for entry in entries.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<i64>() else {
            continue;
        };
        if pid <= 1 {
            continue;
        }
        let Ok(content) = std::fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        let text = String::from_utf8_lossy(&content).replace('\0', " ");
        if text.contains(pattern) {
            return Some(pid);
        }
    }
    None
}

/// 轮询等待进程出现:服务启动是异步的,`spawn` 返回后子进程可能尚未完成
/// exec,立即扫描 `/proc` 会错过。最多等待约 5 秒。
pub(crate) fn wait_for_command_pid(pattern: &str) -> Option<i64> {
    for _ in 0..50 {
        if let Some(pid) = pid_of_command(pattern) {
            return Some(pid);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}
