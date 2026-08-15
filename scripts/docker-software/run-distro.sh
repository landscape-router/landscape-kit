#!/usr/bin/env bash
# 单个发行版容器内的常用软件（Docker）安装验证。
#
# 真实执行的部分：发行版检测、仓库文件写入、GPG key 下载与 dearmor、软件包管理器
# 真实安装 docker-ce、服务启用命令与最终 daemon 验证调用。容器内没有 systemd PID 1
# 也无法运行真实 dockerd，因此 systemctl 与 docker 使用记录型 shim：调用参数写入
# 日志供断言；真实服务启停与 daemon 运行属于宿主行为，不在此层重复。
set -euo pipefail

distro=${1:?usage: run-distro.sh <debian|ubuntu|fedora|archlinux>}
lkit=/usr/local/bin/lkit

fail() {
  echo "FAIL($distro): $*" >&2
  exit 1
}
ok() {
  echo "PASS($distro): $*"
}

assert_contains() {
  local file=$1 needle=$2
  grep -qF "$needle" "$file" || fail "expected [$needle] in $(basename "$file"): $(tr '\n' '|' <"$file")"
}
assert_log_contains() {
  local file=$1 needle=$2
  grep -qF "$needle" "$file" || fail "expected [$needle] in $file: $(tr '\n' '|' <"$file")"
}

# 架构映射：与 lkit 的 apt 仓库 arch 映射一致（x86_64 -> amd64，aarch64 -> arm64）。
machine=$(uname -m)
case "$machine" in
  x86_64) apt_arch=amd64 ;;
  aarch64 | arm64) apt_arch=arm64 ;;
  *) fail "unsupported container architecture $machine" ;;
esac

# 安装前的只读契约：软件列表可读且 Docker 未安装，非交互缺来源报用法错误。
"$lkit" software list >/tmp/software-list-before.txt
grep -q "(docker)" /tmp/software-list-before.txt || fail "software list must show docker"
grep -q "not installed" /tmp/software-list-before.txt \
  || fail "docker must be listed as not installed before install"
if "$lkit" software install docker --non-interactive >/dev/null 2>&1; then
  fail "software install docker --non-interactive without --source must be a usage error"
fi
ok "pre-install contract (list, usage error)"

# 服务层 shim：记录 systemctl/docker 调用并成功返回，其余流程全部真实执行。
shim_dir=/opt/lkit-fakes
mkdir -p "$shim_dir" /var/log/lkit-software
cat >"$shim_dir/systemctl" <<'SH'
#!/bin/sh
echo "systemctl $*" >>/var/log/lkit-software/systemctl.log
exit 0
SH
cat >"$shim_dir/docker" <<'SH'
#!/bin/sh
echo "docker $*" >>/var/log/lkit-software/docker.log
exit 0
SH
chmod +x "$shim_dir/systemctl" "$shim_dir/docker"
export PATH="$shim_dir:$PATH"

case "$distro" in
debian)
  source /etc/os-release
  codename=${VERSION_CODENAME:?missing VERSION_CODENAME}
  "$lkit" software install docker --yes --source official
  ok "install docker from the official repository"
  assert_contains /etc/apt/sources.list.d/docker.list \
    "deb [arch=$apt_arch signed-by=/etc/apt/keyrings/docker.gpg] https://download.docker.com/linux/debian $codename stable"
  gpg --batch --show-keys /etc/apt/keyrings/docker.gpg 2>/dev/null | grep -q '^pub' \
    || fail "docker.gpg must be a valid dearmored keyring"
  ok "apt source file and gpg keyring"
  ;;

ubuntu)
  source /etc/os-release
  codename=${VERSION_CODENAME:?missing VERSION_CODENAME}
  "$lkit" software install docker --yes --source ustc
  ok "install docker from the USTC mirror"
  assert_contains /etc/apt/sources.list.d/docker.list \
    "deb [arch=$apt_arch signed-by=/etc/apt/keyrings/docker.gpg] https://mirrors.ustc.edu.cn/docker-ce/linux/ubuntu $codename stable"
  ok "apt source file points at the USTC mirror"
  ;;

fedora)
  source /etc/os-release
  major=${VERSION_ID%%.*}
  # TUNA/USTC 对 docker-ce 的 fedora 仓库存在地域/UA 过滤（非 CN 流量 403），
  # 真实安装矩阵使用官方源与阿里云源；TUNA/USTC 的 URL 映射由单元测试覆盖。
  "$lkit" software install docker --yes --source aliyun
  ok "install docker from the Aliyun mirror"
  assert_contains /etc/yum.repos.d/docker-ce.repo \
    "baseurl=https://mirrors.aliyun.com/docker-ce/linux/fedora/$major/\$basearch/stable"
  assert_contains /etc/yum.repos.d/docker-ce.repo \
    "gpgkey=https://mirrors.aliyun.com/docker-ce/linux/fedora/gpg"
  ok "dnf repo file points at the Aliyun mirror"
  ;;

archlinux)
  # pacman 从 Arch 官方仓库安装，不写第三方仓库；来源参数仍必须被接受。
  "$lkit" software install docker --yes --source ustc
  ok "install docker from pacman (source ustc accepted)"
  pacman -Q docker >/dev/null || fail "docker package must be installed"
  ;;

*)
  fail "unknown distro $distro"
  ;;
esac

# 安装后的公共断言：真实二进制、服务启用与 daemon 验证契约、状态刷新。
[ -x /usr/bin/docker ] || fail "/usr/bin/docker must exist after install"
assert_log_contains /var/log/lkit-software/systemctl.log "enable --now docker"
assert_log_contains /var/log/lkit-software/docker.log "info"
"$lkit" software list >/tmp/software-list-after.txt
grep -q "installed" /tmp/software-list-after.txt \
  || fail "docker must be listed as installed after install"
ok "$distro: docker install verified"
