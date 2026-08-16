use std::fs;
use std::path::Path;

use crate::mirror::{Family, Host};
use crate::software::{DockerSource, InstallPhase, SoftwareError, paths};

/// apt 家族（Debian/Ubuntu）安装 docker-ce 的软件包列表。
const APT_PACKAGES: [&str; 5] = [
    "docker-ce",
    "docker-ce-cli",
    "containerd.io",
    "docker-buildx-plugin",
    "docker-compose-plugin",
];

/// dnf 家族（Fedora/RHEL 系）安装 docker-ce 的软件包列表。
const DNF_PACKAGES: [&str; 5] = [
    "docker-ce",
    "docker-ce-cli",
    "containerd.io",
    "docker-buildx-plugin",
    "docker-compose-plugin",
];

/// pacman（Arch）安装 docker 的软件包列表。
const PACMAN_PACKAGES: [&str; 3] = ["docker", "docker-buildx", "docker-compose"];

/// 按发行版家族安装 Docker。
pub(crate) fn install(
    host: &Host,
    source: DockerSource,
    stream: bool,
    phase: &mut dyn FnMut(InstallPhase),
) -> Result<(), SoftwareError> {
    match host.family {
        Family::Debian | Family::Ubuntu => install_apt(host, source, stream, phase),
        Family::Fedora | Family::Rocky | Family::Alma => install_dnf(host, source, stream, phase),
        Family::Arch => install_pacman(stream, phase),
    }
}

/// apt（Debian/Ubuntu）：预置依赖、下载并 dearmor GPG key、写入
/// `/etc/apt/sources.list.d/docker.list`，`apt-get update` 后安装软件包。
fn install_apt(
    host: &Host,
    source: DockerSource,
    stream: bool,
    phase: &mut dyn FnMut(InstallPhase),
) -> Result<(), SoftwareError> {
    phase(InstallPhase::Preparing);
    // 基础镜像可能没有包列表：先刷新一次，再安装预置依赖。
    run_command("apt-get", &["update", "-y"], stream)?;
    run_command(
        "apt-get",
        &["install", "-y", "ca-certificates", "curl", "gnupg"],
        stream,
    )?;
    let keyrings = paths().apt_keyrings_dir.clone();
    fs::create_dir_all(&keyrings)?;
    let slug = crate::software::docker_family_slug(host.family);
    let gpg_url = format!("{}/linux/{slug}/gpg", source.base_url());
    let armored = keyrings.join(".docker-archive.key");
    let keyring = keyrings.join("docker.gpg");
    let armored_path = armored.to_string_lossy().to_string();
    run_command("curl", &["-fsSL", &gpg_url, "-o", &armored_path], stream)?;
    dearmor_gpg(&armored, &keyring)?;
    let _ = fs::remove_file(&armored);
    write_apt_source(host, source)?;
    phase(InstallPhase::InstallingPackages);
    run_command("apt-get", &["update", "-y"], stream)?;
    let mut args: Vec<&str> = vec!["install", "-y"];
    args.extend(APT_PACKAGES);
    run_command("apt-get", &args, stream)?;
    finish(stream, phase)
}

/// 写入 apt 的 docker-ce 源文件（纯文件操作，可单测）。
fn write_apt_source(host: &Host, source: DockerSource) -> Result<(), SoftwareError> {
    let codename = host.codename.clone().ok_or_else(|| {
        SoftwareError::Message(crate::tr!(crate::keys::SOFTWARE_MISSING_CODENAME))
    })?;
    let slug = crate::software::docker_family_slug(host.family);
    let arch = apt_arch()?;
    let keyring = paths().apt_keyrings_dir.join("docker.gpg");
    let list = format!(
        "deb [arch={arch} signed-by={}] {}/linux/{slug} {codename} stable\n",
        keyring.display(),
        source.base_url()
    );
    let sources_dir = paths().apt_sources_list_d.clone();
    fs::create_dir_all(&sources_dir)?;
    crate::mirror::common::write_atomic(&sources_dir.join("docker.list"), &list)
        .map_err(|error| SoftwareError::Message(error.to_string()))
}

/// dnf（Fedora/Rocky/AlmaLinux）：写入 `/etc/yum.repos.d/docker-ce.repo`，
/// 安装软件包后启用服务。
fn install_dnf(
    host: &Host,
    source: DockerSource,
    stream: bool,
    phase: &mut dyn FnMut(InstallPhase),
) -> Result<(), SoftwareError> {
    phase(InstallPhase::Preparing);
    write_dnf_repo(host, source)?;
    phase(InstallPhase::InstallingPackages);
    let mut args: Vec<&str> = vec!["install", "-y"];
    args.extend(DNF_PACKAGES);
    run_command("dnf", &args, stream)?;
    finish(stream, phase)
}

/// 写入 dnf/yum 的 docker-ce 仓库文件（纯文件操作，可单测）。
fn write_dnf_repo(host: &Host, source: DockerSource) -> Result<(), SoftwareError> {
    let slug = crate::software::docker_family_slug(host.family);
    let version = crate::software::detect::major_version(&paths().os_release).ok_or_else(|| {
        SoftwareError::Message(crate::tr!(crate::keys::SOFTWARE_OS_RELEASE_UNREADABLE))
    })?;
    let base = source.base_url();
    let repo = format!(
        "[docker-ce-stable]\n\
         name=Docker CE Stable - $basearch\n\
         baseurl={base}/linux/{slug}/{version}/$basearch/stable\n\
         enabled=1\n\
         gpgcheck=1\n\
         gpgkey={base}/linux/{slug}/gpg\n"
    );
    fs::create_dir_all(&paths().dnf_repos_dir)?;
    crate::mirror::common::write_atomic(&paths().dnf_repos_dir.join("docker-ce.repo"), &repo)
        .map_err(|error| SoftwareError::Message(error.to_string()))
}

/// pacman（Arch）：官方仓库直接安装。
fn install_pacman(stream: bool, phase: &mut dyn FnMut(InstallPhase)) -> Result<(), SoftwareError> {
    phase(InstallPhase::Preparing);
    phase(InstallPhase::InstallingPackages);
    let mut args: Vec<&str> = vec!["-Sy", "--noconfirm"];
    args.extend(PACMAN_PACKAGES);
    run_command("pacman", &args, stream)?;
    finish(stream, phase)
}

/// 启用并启动 docker 服务，并做最终功能验证。
fn finish(stream: bool, phase: &mut dyn FnMut(InstallPhase)) -> Result<(), SoftwareError> {
    phase(InstallPhase::StartingService);
    match run_command("systemctl", &["enable", "--now", "docker"], stream) {
        Ok(()) => {}
        Err(SoftwareError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            run_command("service", &["docker", "start"], stream)?;
        }
        Err(error) => return Err(error),
    }
    // 最终验证：daemon 未就绪时安装不能视为成功。
    run_command("docker", &["info"], false)
        .map_err(|_| SoftwareError::Message(crate::tr!(crate::keys::SOFTWARE_SERVICE_NOT_RUNNING)))
}

/// 运行外部命令。`stream` 为 true 时继承 stdio 并透出原始输出；
/// 否则捕获输出，仅在失败时把 stderr 并入错误信息。
fn run_command(program: &str, args: &[&str], stream: bool) -> Result<(), SoftwareError> {
    let mut command = std::process::Command::new(program);
    command.args(args);
    if stream {
        let status = command.status()?;
        if status.success() {
            Ok(())
        } else {
            Err(SoftwareError::Message(format!(
                "{program} exited with status {status}"
            )))
        }
    } else {
        let output = command.output()?;
        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stderr = stderr.trim();
            Err(SoftwareError::Message(if stderr.is_empty() {
                format!("{program} exited with status {}", output.status)
            } else {
                format!("{program}: {stderr}")
            }))
        }
    }
}

/// 把 ASCII-armored GPG key 转成二进制 keyring 写入 `output`。
fn dearmor_gpg(armored: &Path, output: &Path) -> Result<(), SoftwareError> {
    let input = fs::File::open(armored)?;
    let file = fs::File::create(output)?;
    let status = std::process::Command::new("gpg")
        .arg("--batch")
        .arg("--dearmor")
        .stdin(std::process::Stdio::from(input))
        .stdout(std::process::Stdio::from(file))
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(SoftwareError::Message(format!(
            "gpg --dearmor exited with status {status}"
        )))
    }
}

/// 把 `std::env::consts::ARCH` 映射为 apt 架构名。
fn apt_arch() -> Result<&'static str, SoftwareError> {
    match std::env::consts::ARCH {
        "x86_64" => Ok("amd64"),
        "aarch64" => Ok("arm64"),
        "arm" => Ok("armhf"),
        other => Err(SoftwareError::Message(crate::tr!(
            crate::keys::SOFTWARE_ARCH_UNSUPPORTED,
            arch = other
        ))),
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use std::fs;

    use crate::mirror::{Family, Host};
    use crate::software::test_support::TestPathsGuard;
    use crate::software::{DockerSource, Software, SoftwarePaths};

    fn test_paths() -> (SoftwarePaths, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "lkit-software-docker-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let paths = SoftwarePaths {
            os_release: root.join("os-release"),
            apt_keyrings_dir: root.join("etc/apt/keyrings"),
            apt_sources_list_d: root.join("etc/apt/sources.list.d"),
            dnf_repos_dir: root.join("etc/yum.repos.d"),
            docker_bin: vec![root.join("usr/bin/docker")],
            allow_non_root: true,
        };
        (paths, root)
    }

    fn write_source(path: &std::path::Path, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn apt_source_file_contains_official_url() {
        let (paths, root) = test_paths();
        let guard = TestPathsGuard::set(paths);
        let host = Host {
            family: Family::Debian,
            codename: Some("bookworm".into()),
        };
        super::write_apt_source(&host, DockerSource::Official).unwrap();
        let list = fs::read_to_string(root.join("etc/apt/sources.list.d/docker.list")).unwrap();
        assert_eq!(
            list,
            format!(
                "deb [arch=amd64 signed-by={}] https://download.docker.com/linux/debian bookworm stable\n",
                root.join("etc/apt/keyrings/docker.gpg").display()
            )
        );
        drop(guard);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn apt_source_file_uses_selected_mirror() {
        let (paths, root) = test_paths();
        let guard = TestPathsGuard::set(paths);
        let host = Host {
            family: Family::Ubuntu,
            codename: Some("jammy".into()),
        };
        super::write_apt_source(&host, DockerSource::Tuna).unwrap();
        let list = fs::read_to_string(root.join("etc/apt/sources.list.d/docker.list")).unwrap();
        assert!(
            list.contains(
                "https://mirrors.tuna.tsinghua.edu.cn/docker-ce/linux/ubuntu jammy stable"
            ),
            "got: {list}"
        );
        drop(guard);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn apt_source_file_uses_tencent_and_huawei_mirror() {
        let (paths, root) = test_paths();
        let guard = TestPathsGuard::set(paths);
        let host = Host {
            family: Family::Ubuntu,
            codename: Some("noble".into()),
        };
        super::write_apt_source(&host, DockerSource::Tencent).unwrap();
        let list = fs::read_to_string(root.join("etc/apt/sources.list.d/docker.list")).unwrap();
        assert!(
            list.contains("https://mirrors.cloud.tencent.com/docker-ce/linux/ubuntu noble stable"),
            "got: {list}"
        );
        super::write_apt_source(&host, DockerSource::Huawei).unwrap();
        let list = fs::read_to_string(root.join("etc/apt/sources.list.d/docker.list")).unwrap();
        assert!(
            list.contains("https://mirrors.huaweicloud.com/docker-ce/linux/ubuntu noble stable"),
            "got: {list}"
        );
        drop(guard);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn apt_source_requires_codename() {
        let (paths, root) = test_paths();
        let guard = TestPathsGuard::set(paths);
        let host = Host {
            family: Family::Debian,
            codename: None,
        };
        assert!(super::write_apt_source(&host, DockerSource::Official).is_err());
        drop(guard);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn dnf_repo_file_contains_official_url_and_version() {
        let (paths, root) = test_paths();
        let guard = TestPathsGuard::set(paths);
        write_source(
            &root.join("os-release"),
            "ID=\"rocky\"\nVERSION_ID=\"9.3\"\n",
        );
        let host = Host {
            family: Family::Rocky,
            codename: None,
        };
        super::write_dnf_repo(&host, DockerSource::Official).unwrap();
        let repo = fs::read_to_string(root.join("etc/yum.repos.d/docker-ce.repo")).unwrap();
        assert!(repo.contains("https://download.docker.com/linux/rocky/9/$basearch/stable"));
        assert!(repo.contains("gpgkey=https://download.docker.com/linux/rocky/gpg"));
        drop(guard);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn dnf_repo_file_uses_selected_mirror() {
        let (paths, root) = test_paths();
        let guard = TestPathsGuard::set(paths);
        write_source(
            &root.join("os-release"),
            "ID=\"rocky\"\nVERSION_ID=\"9.3\"\n",
        );
        let host = Host {
            family: Family::Rocky,
            codename: None,
        };
        super::write_dnf_repo(&host, DockerSource::Aliyun).unwrap();
        let repo = fs::read_to_string(root.join("etc/yum.repos.d/docker-ce.repo")).unwrap();
        assert!(
            repo.contains("https://mirrors.aliyun.com/docker-ce/linux/rocky/9/$basearch/stable")
        );
        drop(guard);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn dnf_repo_requires_version() {
        let (paths, root) = test_paths();
        let guard = TestPathsGuard::set(paths);
        write_source(&root.join("os-release"), "ID=\"fedora\"\n");
        let host = Host {
            family: Family::Fedora,
            codename: None,
        };
        assert!(super::write_dnf_repo(&host, DockerSource::Official).is_err());
        drop(guard);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn apt_arch_maps_common_architectures() {
        assert!(super::apt_arch().is_ok());
    }

    #[test]
    fn docker_installed_detects_existing_binary() {
        let (paths, root) = test_paths();
        let guard = TestPathsGuard::set(paths);
        let binary = root.join("usr/bin/docker");
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(&binary, "#!/bin/sh\n").unwrap();
        assert!(Software::Docker.installed());
        fs::remove_file(&binary).unwrap();
        assert!(!Software::Docker.installed());
        drop(guard);
        let _ = fs::remove_dir_all(&root);
    }
}
