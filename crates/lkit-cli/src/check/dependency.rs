use std::fs::Metadata;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

use super::model::{CheckResult, Status};

pub fn run() -> Vec<CheckResult> {
    vec![iproute2(), tc(), pppd(), container_runtime()]
}

fn iproute2() -> CheckResult {
    let result = CheckResult::new("dependency.iproute2", "ip 命令");
    match find_in_path("ip") {
        Some(path) => result.set(
            Status::Pass,
            path.display().to_string(),
            "ip 命令存在且可执行",
        ),
        None => result
            .set(
                Status::Error,
                "未找到",
                "未找到 ip 命令（属于 iproute2 软件包）",
            )
            .suggestion("安装 iproute2：apt install iproute2"),
    }
}

fn tc() -> CheckResult {
    let mut result = CheckResult::new("dependency.tc", "tc 命令与 BPF 支持");
    let Some(path) = find_in_path("tc") else {
        return result
            .set(
                Status::Error,
                "未找到",
                "未找到 tc 命令（属于 iproute2 软件包）",
            )
            .suggestion("安装 iproute2：apt install iproute2");
    };
    let path_display = path.display().to_string();
    result.value = path_display.clone();
    match Command::new(&path).args(["filter", "help"]).output() {
        Ok(output) => {
            let mut text = String::from_utf8_lossy(&output.stdout).to_lowercase();
            text.push_str(&String::from_utf8_lossy(&output.stderr).to_lowercase());
            if output.status.success() && text.contains("bpf") {
                result.set(Status::Pass, path_display, "tc 存在且支持 BPF 过滤器")
            } else if !output.status.success() {
                result.set(
                    Status::Unknown,
                    path_display,
                    format!(
                        "tc filter help 执行失败（退出码 {:?}）",
                        output.status.code()
                    ),
                )
            } else {
                result
                    .set(
                        Status::Error,
                        path_display,
                        "tc 帮助文本中未包含 bpf，BPF 支持不可用",
                    )
                    .suggestion("升级 iproute2 或安装支持 BPF 的版本")
            }
        }
        Err(err) => result.set(
            Status::Unknown,
            path_display,
            format!("无法执行 tc filter help：{err}"),
        ),
    }
}

fn pppd() -> CheckResult {
    let result = CheckResult::new("dependency.pppd", "pppd 命令");
    match find_in_path("pppd") {
        Some(path) => result.set(
            Status::Pass,
            path.display().to_string(),
            "pppd 命令存在且可执行（用于 PPPoE 拨号）",
        ),
        None => result
            .set(
                Status::Error,
                "未找到",
                "未找到 pppd 命令（用于 PPPoE 拨号）",
            )
            .suggestion("安装 PPP：apt install ppp"),
    }
}

fn container_runtime() -> CheckResult {
    let result = CheckResult::new("dependency.container_runtime", "容器运行时（软依赖）");
    let found = ["docker", "podman"]
        .iter()
        .find_map(|name| find_in_path(name));
    match found {
        Some(path) => result.set(
            Status::Pass,
            path.display().to_string(),
            "docker 或 podman 可用",
        ),
        None => result
            .set(Status::Warning, "未找到", "docker 与 podman 均不可用")
            .suggestion(
                "缺少容器运行时不阻断基础部署；需要将流量分流到容器时，必须安装并配置 Docker 或 Podman",
            ),
    }
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let candidate = dir.join(name);
        std::fs::metadata(&candidate)
            .ok()
            .filter(is_executable)
            .map(|_| candidate)
    })
}

fn is_executable(metadata: &Metadata) -> bool {
    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
}
