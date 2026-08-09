#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
installer=$repo_root/scripts/install-lkit.sh
test_root=$(mktemp -d)
fixture_directory=$test_root/fixtures
fake_bin=$test_root/bin
wget_bin=$test_root/wget-bin
plain_bin=$test_root/plain-bin
restricted_bin=$test_root/restricted-bin
install_directory=$test_root/install
destination=$install_directory/lkit
log=$test_root/lkit-args.log

cleanup() {
  rm -rf "$test_root"
}
trap cleanup EXIT

fail() {
  echo "install-lkit test: $*" >&2
  exit 1
}

mkdir -p "$fixture_directory" "$fake_bin" "$wget_bin" "$plain_bin" "$restricted_bin" "$install_directory"

for tool in awk bash cp dirname install mktemp mv rm sha256sum sh; do
  ln -s "$(command -v "$tool")" "$restricted_bin/$tool"
done

cat >"$fake_bin/id" <<'SH'
#!/usr/bin/env bash
if [[ ${1:-} == -u ]]; then
  printf '%s\n' "${FAKE_ID_U:-0}"
else
  exec /usr/bin/id "$@"
fi
SH

cat >"$fake_bin/uname" <<'SH'
#!/usr/bin/env bash
case ${1:-} in
  -s) printf '%s\n' "${FAKE_UNAME_S:-Linux}" ;;
  -m) printf '%s\n' "${FAKE_UNAME_M:-x86_64}" ;;
  *) exec /usr/bin/uname "$@" ;;
esac
SH

cat >"$fake_bin/ldd" <<'SH'
#!/usr/bin/env bash
if [[ ${FAKE_LDD_FLAVOR:-glibc} == musl ]]; then
  printf 'musl libc (x86_64)\n' >&2
  exit 1
fi
printf 'ldd (GNU libc) 2.36\n'
SH

cat >"$fake_bin/curl" <<'SH'
#!/usr/bin/env bash
set -euo pipefail

output=
url=
while (($# > 0)); do
  case $1 in
    --proto | --proto-redir | --output)
      [[ $1 != --output ]] || output=$2
      shift 2
      ;;
    --tlsv1.2 | --fail | --silent | --show-error | --location)
      shift
      ;;
    https://*)
      url=$1
      shift
      ;;
    *)
      echo "fake curl: unsupported argument $1" >&2
      exit 2
      ;;
  esac
done

asset=${url##*/}
if [[ ${FAKE_CURL_FAIL_ASSET:-} == "$asset" ]]; then
  exit 22
fi
cp "$FAKE_FIXTURE_DIRECTORY/$asset" "$output"
if [[ ${FAKE_CURL_CORRUPT_ASSET:-} == "$asset" ]]; then
  printf 'corrupt' >>"$output"
fi
SH

cat >"$wget_bin/wget" <<'SH'
#!/usr/bin/env bash
set -euo pipefail

output=
url=
while (($# > 0)); do
  case $1 in
    --output-document)
      output=$2
      shift 2
      ;;
    --https-only | --no-verbose | --secure-protocol=*)
      shift
      ;;
    https://*)
      url=$1
      shift
      ;;
    *)
      echo "fake wget: unsupported argument $1" >&2
      exit 2
      ;;
  esac
done

asset=${url##*/}
if [[ ${FAKE_WGET_FAIL_ASSET:-} == "$asset" ]]; then
  exit 8
fi
cp "$FAKE_FIXTURE_DIRECTORY/$asset" "$output"
if [[ ${FAKE_WGET_CORRUPT_ASSET:-} == "$asset" ]]; then
  printf 'corrupt' >>"$output"
fi
SH

cat >"$fixture_directory/lkit-x86_64" <<'SH'
#!/bin/sh
case "${1:-}" in
  --version) printf 'lkit 0.1.0\n' ;;
  --fixture-architecture) printf 'x86_64\n' ;;
  install) printf '%s\n' "$@" >"$LKIT_TEST_LOG" ;;
  *) exit 2 ;;
esac
SH

cat >"$fixture_directory/lkit-aarch64" <<'SH'
#!/bin/sh
case "${1:-}" in
  --version) printf 'lkit 0.1.0\n' ;;
  --fixture-architecture) printf 'aarch64\n' ;;
  install) printf '%s\n' "$@" >"$LKIT_TEST_LOG" ;;
  *) exit 2 ;;
esac
SH

chmod 0755 "$fake_bin/id" "$fake_bin/uname" "$fake_bin/ldd" "$fake_bin/curl" \
  "$wget_bin/wget" \
  "$fixture_directory/lkit-x86_64" "$fixture_directory/lkit-aarch64"
cp "$fake_bin/id" "$fake_bin/uname" "$fake_bin/ldd" "$wget_bin"
cp "$fake_bin/id" "$fake_bin/uname" "$fake_bin/ldd" "$plain_bin"
(
  cd "$fixture_directory"
  sha256sum lkit-aarch64 lkit-x86_64 >SHA256SUMS
)

run_installer() {
  run_path="$fake_bin:$PATH"
  case ${1:-} in
    wget)
      run_path="$wget_bin:$plain_bin:$restricted_bin"
      shift
      ;;
    none)
      run_path="$plain_bin:$restricted_bin"
      shift
      ;;
  esac
  env \
    PATH="$run_path" \
    FAKE_FIXTURE_DIRECTORY="$fixture_directory" \
    FAKE_ID_U="${FAKE_ID_U:-0}" \
    FAKE_UNAME_M="${FAKE_UNAME_M:-x86_64}" \
    FAKE_UNAME_S="${FAKE_UNAME_S:-Linux}" \
    FAKE_LDD_FLAVOR="${FAKE_LDD_FLAVOR:-glibc}" \
    FAKE_CURL_FAIL_ASSET="${FAKE_CURL_FAIL_ASSET:-}" \
    FAKE_CURL_CORRUPT_ASSET="${FAKE_CURL_CORRUPT_ASSET:-}" \
    FAKE_WGET_FAIL_ASSET="${FAKE_WGET_FAIL_ASSET:-}" \
    FAKE_WGET_CORRUPT_ASSET="${FAKE_WGET_CORRUPT_ASSET:-}" \
    LKIT_INSTALL_PATH="$destination" \
    LKIT_TEST_LOG="$log" \
    sh "$installer" "$@"
}

run_installer >/dev/null
[[ -x $destination ]] || fail "x86_64 binary was not installed"
[[ $($destination --fixture-architecture) == x86_64 ]] \
  || fail "x86_64 selected the wrong asset"

rm -f "$destination"
FAKE_UNAME_M=aarch64 run_installer >/dev/null
[[ $($destination --fixture-architecture) == aarch64 ]] \
  || fail "aarch64 selected the wrong asset"

if FAKE_UNAME_M=riscv64 run_installer >/dev/null 2>&1; then
  fail "unsupported architecture was accepted"
fi
if FAKE_UNAME_S=Darwin run_installer >/dev/null 2>&1; then
  fail "non-Linux platform was accepted"
fi
if musl_output=$(FAKE_LDD_FLAVOR=musl run_installer 2>&1); then
  fail "musl platform was accepted"
fi
[[ $musl_output == *"musl-based distributions"* ]] \
  || fail "musl rejection did not explain the glibc binary requirement"
if FAKE_ID_U=1000 run_installer >/dev/null 2>&1; then
  fail "non-root installation was accepted"
fi
if run_installer switch >/dev/null 2>&1; then
  fail "unsupported installer argument was accepted"
fi

rm -f "$log"
run_installer install --version 1.2.3 --repository >/dev/null
expected_arguments=$'install\n--version\n1.2.3\n--repository'
[[ $(<"$log") == "$expected_arguments" ]] \
  || fail "lkit install arguments were not preserved"

printf 'existing binary\n' >"$destination"
before=$(sha256sum "$destination")
if FAKE_CURL_CORRUPT_ASSET=lkit-x86_64 run_installer >/dev/null 2>&1; then
  fail "corrupted binary was accepted"
fi
[[ $(sha256sum "$destination") == "$before" ]] \
  || fail "checksum failure replaced the existing binary"

if FAKE_CURL_FAIL_ASSET=SHA256SUMS run_installer >/dev/null 2>&1; then
  fail "checksum download failure was accepted"
fi
[[ $(sha256sum "$destination") == "$before" ]] \
  || fail "download failure replaced the existing binary"

rm -f "$destination"
run_installer wget >/dev/null
[[ -x $destination ]] || fail "wget-only installation did not install the binary"
[[ $($destination --fixture-architecture) == x86_64 ]] \
  || fail "wget-only installation selected the wrong asset"

rm -f "$destination"
FAKE_UNAME_M=aarch64 run_installer wget >/dev/null
[[ $($destination --fixture-architecture) == aarch64 ]] \
  || fail "wget-only installation selected the wrong aarch64 asset"

printf 'existing binary\n' >"$destination"
before=$(sha256sum "$destination")
if FAKE_WGET_CORRUPT_ASSET=lkit-x86_64 run_installer wget >/dev/null 2>&1; then
  fail "wget corrupted binary was accepted"
fi
[[ $(sha256sum "$destination") == "$before" ]] \
  || fail "wget checksum failure replaced the existing binary"

if FAKE_WGET_FAIL_ASSET=SHA256SUMS run_installer wget >/dev/null 2>&1; then
  fail "wget checksum download failure was accepted"
fi
[[ $(sha256sum "$destination") == "$before" ]] \
  || fail "wget download failure replaced the existing binary"

if error_output=$(run_installer none 2>&1); then
  fail "installation without curl or wget was accepted"
fi
[[ $error_output == *"curl or wget is required"* ]] \
  || fail "missing downloader error message is unclear"

echo "install-lkit test: passed"
