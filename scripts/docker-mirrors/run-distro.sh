#!/usr/bin/env bash
# 单个发行版容器内的换源验证：官方源 → tuna → 恢复备份 → aliyun → 官方源，
# Debian/Ubuntu 额外验证"仅 CD 源"兜底。失败输出 FAIL 并返回非零。
#
# 注意：`--restore` 只还原上一次换源前的备份，因此 restore 验证紧跟首次切换。
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

# 断言某文件包含/不包含固定字符串。
assert_contains() {
  local file=$1 needle=$2
  grep -qF "$needle" "$file" || fail "expected [$needle] in $(basename "$file"): $(tr '\n' '|' <"$file")"
}
assert_not_contains() {
  local file=$1 needle=$2
  if grep -qF "$needle" "$file"; then
    fail "unexpected [$needle] in $(basename "$file"): $(tr '\n' '|' <"$file")"
  fi
}

# 断言受管源文件集合中存在/不存在固定字符串。
sources_assert() {
  local needle=$1
  shift
  for file in "$@"; do
    [ -f "$file" ] || continue
    if grep -qF "$needle" "$file"; then
      return 0
    fi
  done
  fail "expected [$needle] in managed source files"
}
sources_assert_not() {
  local needle=$1
  shift
  for file in "$@"; do
    [ -f "$file" ] || continue
    if grep -qF "$needle" "$file"; then
      fail "unexpected [$needle] in $(basename "$file")"
    fi
  done
}

case "$distro" in
debian)
  # debian:bookworm 镜像的布局随版本变化：老版本是 sources.list，新版是
  # sources.list.d/debian.sources（deb822），两者都必须兼容。
  backup_dir=/var/lib/lkit/mirror-backup/debian
  declare -A original
  for file in /etc/apt/sources.list /etc/apt/sources.list.d/*; do
    [ -f "$file" ] && original["$file"]="$(cat "$file")"
  done

  "$lkit" set-mirror tuna --yes
  ok "switch to tuna"
  sources_assert "mirrors.tuna.tsinghua.edu.cn/debian" /etc/apt/sources.list /etc/apt/sources.list.d/*
  sources_assert "deb.debian.org/debian-security" /etc/apt/sources.list /etc/apt/sources.list.d/*
  [ -d "$backup_dir" ] || fail "backup directory missing after switch"

  "$lkit" set-mirror --restore --yes
  ok "restore from backup"
  for file in "${!original[@]}"; do
    [ "$(cat "$file")" = "${original[$file]}" ] || fail "restore did not return the original $file"
  done
  [ ! -d "$backup_dir" ] || fail "backup directory must be removed after restore"

  "$lkit" set-mirror aliyun --yes --replace-security
  ok "switch to aliyun with security"
  sources_assert "mirrors.aliyun.com/debian" /etc/apt/sources.list /etc/apt/sources.list.d/*
  sources_assert "mirrors.aliyun.com/debian-security" /etc/apt/sources.list /etc/apt/sources.list.d/*

  "$lkit" set-mirror official --yes
  ok "restore official hosts"
  sources_assert "deb.debian.org/debian" /etc/apt/sources.list /etc/apt/sources.list.d/*
  sources_assert_not "mirrors.tuna.tsinghua.edu.cn/debian" /etc/apt/sources.list /etc/apt/sources.list.d/*
  sources_assert_not "mirrors.aliyun.com/debian" /etc/apt/sources.list /etc/apt/sources.list.d/*

  # 仅 CD 源场景：无任何可识别 URL，换源应转换为镜像并保留 suites/components。
  rm -f /etc/apt/sources.list.d/*
  cat >/etc/apt/sources.list <<'EOF'
deb cdrom:[Debian GNU/Linux 12.5.0 _Bookworm_ - Official amd64 DVD Binary-1 20240210-10:16]/ bookworm contrib main non-free
EOF
  "$lkit" set-mirror tuna --yes
  ok "convert the only CD-ROM source"
  assert_contains /etc/apt/sources.list \
    "deb https://mirrors.tuna.tsinghua.edu.cn/debian bookworm contrib main non-free"
  "$lkit" set-mirror --restore --yes
  assert_contains /etc/apt/sources.list "deb cdrom:[Debian GNU/Linux 12.5.0 _Bookworm_"

  # 空源文件场景：sources.list 为空（无任何条目）→ 合成新条目。
  : >/etc/apt/sources.list
  "$lkit" set-mirror tuna --yes
  ok "synthesize entries for an empty sources.list"
  assert_contains /etc/apt/sources.list \
    "deb https://mirrors.tuna.tsinghua.edu.cn/debian bookworm main contrib non-free"

  # --check：只读格式检查，能报告无法识别的行；干净文件退出 0。
  printf 'deb http://deb.debian.org/debian bookworm main\nnot a source line\n' \
    >/etc/apt/sources.list
  if check_out=$("$lkit" set-mirror --check 2>&1); then
    fail "--check must report format issues"
  fi
  echo "$check_out" | grep -q "line 2" || fail "--check must point at the offending line: $check_out"
  ok "format check detects unrecognized lines"
  printf 'deb http://deb.debian.org/debian bookworm main\n' >/etc/apt/sources.list
  "$lkit" set-mirror --check >/dev/null 2>&1 || fail "--check must pass on clean sources"
  ok "format check passes on clean sources"
  ;;

ubuntu)
  # ubuntu:24.04 使用 deb822（ubuntu.sources），旧版 one-line 布局同样兼容。
  # Ubuntu 的 security 并入主仓库路径，始终随主仓库替换。
  # arm64 等 ports 架构的官方主机是 ports.ubuntu.com、路径 /ubuntu-ports。
  machine=$(uname -m)
  case "$machine" in
    aarch64|arm64|arm*|riscv*|ppc64*|s390x) ubuntu_ports=1 ;;
    *) ubuntu_ports=0 ;;
  esac
  if [ "$ubuntu_ports" = 1 ]; then
    official_main=ports.ubuntu.com
    mirror_path=ubuntu-ports
  else
    official_main=archive.ubuntu.com
    mirror_path=ubuntu
  fi
  backup_dir=/var/lib/lkit/mirror-backup/ubuntu
  declare -A original
  for file in /etc/apt/sources.list /etc/apt/sources.list.d/*; do
    [ -f "$file" ] && original["$file"]="$(cat "$file")"
  done

  "$lkit" set-mirror tuna --yes
  ok "switch to tuna"
  sources_assert "mirrors.tuna.tsinghua.edu.cn/$mirror_path" /etc/apt/sources.list /etc/apt/sources.list.d/*
  sources_assert_not "security.ubuntu.com" /etc/apt/sources.list /etc/apt/sources.list.d/*
  [ -d "$backup_dir" ] || fail "backup directory missing after switch"

  "$lkit" set-mirror --restore --yes
  ok "restore from backup"
  for file in "${!original[@]}"; do
    [ "$(cat "$file")" = "${original[$file]}" ] || fail "restore did not return the original $file"
  done
  [ ! -d "$backup_dir" ] || fail "backup directory must be removed after restore"

  "$lkit" set-mirror aliyun --yes
  ok "switch to aliyun"
  sources_assert "mirrors.aliyun.com/$mirror_path" /etc/apt/sources.list /etc/apt/sources.list.d/*

  "$lkit" set-mirror official --yes
  ok "restore official hosts"
  sources_assert "$official_main/$mirror_path" /etc/apt/sources.list /etc/apt/sources.list.d/*
  sources_assert_not "mirrors.tuna.tsinghua.edu.cn" /etc/apt/sources.list /etc/apt/sources.list.d/*

  # 仅 CD 源场景：x86_64 转 /ubuntu，arm64 等 ports 架构转 /ubuntu-ports。
  rm -f /etc/apt/sources.list.d/*
  cat >/etc/apt/sources.list <<'EOF'
deb cdrom:[Ubuntu 24.04 LTS _Noble Numbat_ - Release amd64 (20240423)]/ noble main restricted
EOF
  "$lkit" set-mirror tuna --yes
  ok "convert the only CD-ROM source"
  assert_contains /etc/apt/sources.list \
    "deb https://mirrors.tuna.tsinghua.edu.cn/$mirror_path noble main restricted"
  "$lkit" set-mirror --restore --yes
  assert_contains /etc/apt/sources.list "deb cdrom:[Ubuntu 24.04"
  ;;

fedora)
  # fedora:latest 的 .repo 使用 download.example 占位主机，先换成规范官方主机，
  # 让测试验证"官方源 → 镜像"的真实映射；另补一个 epel fixture 覆盖 epel 路径。
  for repo in /etc/yum.repos.d/*.repo; do
    sed -i 's#download.example/pub/#download.fedoraproject.org/pub/#' "$repo"
  done
  cat >/etc/yum.repos.d/epel.repo <<'EOF'
[epel]
name=Extra Packages for Enterprise Linux $releasever - $basearch
metalink=https://mirrors.fedoraproject.org/metalink?repo=epel-$releasever&arch=$basearch
#baseurl=https://download.fedoraproject.org/pub/epel/$releasever/Everything/$basearch/
gpgcheck=1
enabled=1
EOF

  repos=(/etc/yum.repos.d/*.repo)
  backup_dir=/var/lib/lkit/mirror-backup/fedora
  declare -A original
  for repo in "${repos[@]}"; do
    original["$repo"]="$(cat "$repo")"
  done

  "$lkit" set-mirror tuna --yes
  ok "switch to tuna"
  # 镜像源保留原始协议（http/https），断言不依赖 scheme。
  sources_assert "mirrors.tuna.tsinghua.edu.cn/fedora" "${repos[@]}"
  sources_assert "mirrors.tuna.tsinghua.edu.cn/epel" "${repos[@]}"
  sources_assert "#lkit-mirror: metalink=" "${repos[@]}"
  [ -d "$backup_dir" ] || fail "backup directory missing after switch"

  "$lkit" set-mirror --restore --yes
  ok "restore from backup"
  for repo in "${!original[@]}"; do
    [ "$(cat "$repo")" = "${original[$repo]}" ] || fail "restore did not return the original $repo"
  done
  [ ! -d "$backup_dir" ] || fail "backup directory must be removed after restore"

  "$lkit" set-mirror aliyun --yes
  ok "switch to aliyun"
  sources_assert "mirrors.aliyun.com/fedora" "${repos[@]}"
  sources_assert "mirrors.aliyun.com/epel" "${repos[@]}"

  "$lkit" set-mirror official --yes
  ok "restore official hosts"
  sources_assert "download.fedoraproject.org/pub/fedora" "${repos[@]}"
  sources_assert "download.fedoraproject.org/pub/epel" "${repos[@]}"
  sources_assert_not "mirrors.tuna.tsinghua.edu.cn/fedora" "${repos[@]}"
  ;;

archlinux)
  mirrorlist=/etc/pacman.d/mirrorlist
  backup_dir=/var/lib/lkit/mirror-backup/arch
  original=$(cat "$mirrorlist")

  "$lkit" set-mirror tuna --yes
  ok "switch to tuna"
  assert_contains "$mirrorlist" "Server = https://mirrors.tuna.tsinghua.edu.cn/archlinux/\$repo/os/\$arch"
  [ "$(grep -c '^Server = ' "$mirrorlist")" = "1" ] || fail "mirrorlist must contain exactly one Server line"
  [ -d "$backup_dir" ] || fail "backup directory missing after switch"

  "$lkit" set-mirror --restore --yes
  ok "restore from backup"
  [ "$(cat "$mirrorlist")" = "$original" ] || fail "restore did not return the original mirrorlist"
  [ ! -d "$backup_dir" ] || fail "backup directory must be removed after restore"

  "$lkit" set-mirror aliyun --yes
  ok "switch to aliyun"
  assert_contains "$mirrorlist" "Server = https://mirrors.aliyun.com/archlinux/\$repo/os/\$arch"

  "$lkit" set-mirror official --yes
  ok "restore official host"
  assert_contains "$mirrorlist" "Server = https://geo.mirror.pkgbuild.com/archlinux/\$repo/os/\$arch"
  ;;

*)
  fail "unknown distro $distro"
  ;;
esac

ok "$distro: mirror switch/restore verified"
