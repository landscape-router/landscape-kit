//! 镜像可用性探测：进入换源面板/交互选择时，并行探测每个镜像站上"当前发行版"
//! 的真实文件（apt 的 `dists/<代号>/Release`、dnf 的 `repodata/repomd.xml`、
//! pacman 的 `core/os/<架构>/core.db`），明确 404 判定为不可用（置灰不可选）；
//! 网络失败/超时/TLS 异常等无法确认的情况判定为未知（可选用，确认时警告）。
//!
//! 只读网络操作，不修改任何文件；探测结果按次缓存（面板会话内重复进入不重探）。

use std::collections::HashMap;
use std::time::Duration;

use reqwest::StatusCode;
use reqwest::blocking::Client;
use reqwest::redirect::Policy;

use super::apt::parse::{is_ports_arch, runtime_arch};
use super::{Family, Host, MirrorName, mirror_host, paths};

/// 单镜像的可用性状态。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MirrorStatus {
    /// 探测命中（镜像提供当前发行版的仓库路径）。
    Available,
    /// 明确 404：该镜像不提供当前发行版仓库。
    Unavailable,
    /// 探测失败（离线/超时/TLS 等），无法确认。
    Unknown,
}

/// 探测超时：每个 URL 最多等待 2 秒；并行探测所有镜像，总耗时接近单次超时。
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// 探测单个镜像对当前主机的可用性。`Official` 恒为可用（不探测）。
pub(crate) fn probe(host: &Host, mirror: MirrorName) -> MirrorStatus {
    if mirror == MirrorName::Official {
        return MirrorStatus::Available;
    }
    let Some(urls) = probe_urls(host, mirror) else {
        // 无法解析探测目标（缺 apt 代号/dnf VERSION_ID）→ 未知，不误伤。
        return MirrorStatus::Unknown;
    };
    let Ok(client) = Client::builder()
        .timeout(PROBE_TIMEOUT)
        .redirect(Policy::limited(5))
        .user_agent(concat!("lkit/", env!("CARGO_PKG_VERSION")))
        .build()
    else {
        return MirrorStatus::Unknown;
    };
    let mut any_available = false;
    for url in &urls {
        match check(&client, url) {
            MirrorStatus::Available => any_available = true,
            MirrorStatus::Unavailable => return MirrorStatus::Unavailable,
            MirrorStatus::Unknown => {}
        }
    }
    if any_available {
        MirrorStatus::Available
    } else {
        MirrorStatus::Unknown
    }
}

/// 并行探测全部镜像（`Official` 不探测，恒为可用）。失败路径单次最多约 2 秒。
pub(crate) fn probe_all(host: &Host) -> HashMap<MirrorName, MirrorStatus> {
    let handles: Vec<_> = MirrorName::all()
        .into_iter()
        .filter(|mirror| *mirror != MirrorName::Official)
        .map(|mirror| {
            let host = host.clone();
            std::thread::spawn(move || (mirror, probe(&host, mirror)))
        })
        .collect();
    let mut statuses: HashMap<_, _> = handles
        .into_iter()
        .filter_map(|handle| handle.join().ok())
        .collect();
    statuses.insert(MirrorName::Official, MirrorStatus::Available);
    statuses
}

/// 对单个 URL 做 HEAD 探测：404 → 不可用；其他 4xx/5xx 或网络错误 → 未知；
/// 2xx/3xx（重定向已跟随）→ 可用。
fn check(client: &Client, url: &str) -> MirrorStatus {
    match client.head(url).send() {
        Ok(response) => match response.status() {
            StatusCode::NOT_FOUND => MirrorStatus::Unavailable,
            StatusCode::OK | StatusCode::MOVED_PERMANENTLY | StatusCode::FOUND => {
                MirrorStatus::Available
            }
            // 403（WAF 拦截）与 5xx 无法确认：视为未知而非不可用，避免误伤。
            _ => MirrorStatus::Unknown,
        },
        Err(_) => MirrorStatus::Unknown,
    }
}

/// 为当前主机构建每个镜像的探测 URL 列表。需要发行版代号（apt）或
/// `VERSION_ID`（dnf 的 `$releasever`）；缺任一必需输入时返回 `None`
/// （探测目标无法确定 → 未知）。
fn probe_urls(host: &Host, mirror: MirrorName) -> Option<Vec<String>> {
    let target = mirror_host(mirror)?;
    let arch = runtime_arch();
    urls_for(
        host.family,
        host.codename.as_deref(),
        &arch,
        releasever().as_deref(),
        target,
    )
}

/// 纯函数版本的探测 URL 构造（可单测）。`family` 之外的全部输入均为可选：
/// 缺 apt 代号或 dnf `VERSION_ID` 时返回 `None`。
fn urls_for(
    family: Family,
    codename: Option<&str>,
    arch: &str,
    releasever: Option<&str>,
    target: &str,
) -> Option<Vec<String>> {
    match family {
        Family::Debian => {
            let codename = codename?;
            Some(vec![format!(
                "https://{target}/debian/dists/{codename}/Release"
            )])
        }
        Family::Ubuntu => {
            let codename = codename?;
            let repo = if is_ports_arch(arch) {
                "ubuntu-ports"
            } else {
                "ubuntu"
            };
            Some(vec![format!(
                "https://{target}/{repo}/dists/{codename}/Release"
            )])
        }
        Family::Fedora => {
            // 探测的正是换源后写入的 URL（官方后缀 `/linux/releases/...` 保留，
            // 部分镜像站如 USTC 靠 301 兼容该路径）；EPEL 的版本号与 Fedora 主
            // 版本不对应（epel 只有 8/9/10/next），不作为探测目标。
            let major = releasever?.split('.').next()?.to_string();
            Some(vec![format!(
                "https://{target}/fedora/linux/releases/{major}/Everything/{}/os/repodata/repomd.xml",
                dnf_arch(arch)
            )])
        }
        Family::Rocky => {
            let major = releasever?.split('.').next()?.to_string();
            Some(vec![format!(
                "https://{target}/rockylinux/{major}/BaseOS/{}/os/repodata/repomd.xml",
                dnf_arch(arch)
            )])
        }
        Family::Alma => {
            let major = releasever?.split('.').next()?.to_string();
            Some(vec![format!(
                "https://{target}/almalinux/{major}/BaseOS/{}/os/repodata/repomd.xml",
                dnf_arch(arch)
            )])
        }
        Family::Arch => Some(vec![format!(
            "https://{target}/archlinux/core/os/{arch}/core.db"
        )]),
    }
}

/// dnf 的 basearch：`uname -m` 的 machine 一般一致，仅 Fedora 的 armv7l 例外。
fn dnf_arch(arch: &str) -> &str {
    match arch {
        "armv7l" => "armhfp",
        other => other,
    }
}

/// 从 `/etc/os-release` 读取原始 `VERSION_ID`（`9.4` 等，主版本拆分在
/// [`urls_for`] 内完成）。
fn releasever() -> Option<String> {
    let content = std::fs::read_to_string(&paths().os_release).ok()?;
    content.lines().find_map(|line| {
        let value = line.strip_prefix("VERSION_ID=")?;
        let value = value.trim().trim_matches('"');
        Some(value.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mirror::{Family, Host};

    fn host(family: Family, codename: Option<&str>) -> Host {
        Host {
            family,
            codename: codename.map(String::from),
        }
    }

    #[test]
    fn debian_probes_the_codename_release_file() {
        let urls = urls_for(
            Family::Debian,
            Some("trixie"),
            "x86_64",
            None,
            "mirror.nju.edu.cn",
        )
        .unwrap();
        assert_eq!(
            urls,
            vec!["https://mirror.nju.edu.cn/debian/dists/trixie/Release"]
        );
    }

    #[test]
    fn debian_without_codename_is_unknown() {
        assert_eq!(
            urls_for(Family::Debian, None, "x86_64", None, "mirror.nju.edu.cn"),
            None
        );
    }

    #[test]
    fn ubuntu_ports_arch_probes_ubuntu_ports() {
        let ports = urls_for(
            Family::Ubuntu,
            Some("noble"),
            "aarch64",
            None,
            "mirror.nju.edu.cn",
        )
        .unwrap();
        assert_eq!(
            ports,
            vec!["https://mirror.nju.edu.cn/ubuntu-ports/dists/noble/Release"]
        );
        let x86 = urls_for(
            Family::Ubuntu,
            Some("noble"),
            "x86_64",
            None,
            "mirror.nju.edu.cn",
        )
        .unwrap();
        assert_eq!(
            x86,
            vec!["https://mirror.nju.edu.cn/ubuntu/dists/noble/Release"]
        );
    }

    #[test]
    fn fedora_probes_the_rewritten_release_repomd() {
        let urls = urls_for(
            Family::Fedora,
            None,
            "armv7l",
            Some("42"),
            "mirrors.bfsu.edu.cn",
        )
        .unwrap();
        assert_eq!(
            urls,
            vec![
                "https://mirrors.bfsu.edu.cn/fedora/linux/releases/42/Everything/armhfp/os/repodata/repomd.xml"
            ]
        );
    }

    #[test]
    fn rocky_and_alma_use_releasever_major() {
        let rocky = urls_for(
            Family::Rocky,
            None,
            "x86_64",
            Some("9.4"),
            "mirror.lzu.edu.cn",
        )
        .unwrap();
        assert_eq!(
            rocky,
            vec!["https://mirror.lzu.edu.cn/rockylinux/9/BaseOS/x86_64/os/repodata/repomd.xml"]
        );
        let alma = urls_for(
            Family::Alma,
            None,
            "x86_64",
            Some("9"),
            "mirrors.hust.edu.cn",
        )
        .unwrap();
        assert_eq!(
            alma,
            vec!["https://mirrors.hust.edu.cn/almalinux/9/BaseOS/x86_64/os/repodata/repomd.xml"]
        );
    }

    #[test]
    fn arch_probes_the_core_database() {
        let urls = urls_for(Family::Arch, None, "x86_64", None, "mirrors.zju.edu.cn").unwrap();
        assert_eq!(
            urls,
            vec!["https://mirrors.zju.edu.cn/archlinux/core/os/x86_64/core.db"]
        );
    }

    #[test]
    fn dnf_families_without_releasever_are_unknown() {
        for family in [Family::Rocky, Family::Alma, Family::Fedora] {
            assert_eq!(urls_for(family, None, "x86_64", None, "x.example"), None);
        }
    }

    #[test]
    fn official_is_never_probed() {
        let host = host(Family::Debian, Some("trixie"));
        assert_eq!(probe(&host, MirrorName::Official), MirrorStatus::Available);
    }
}
