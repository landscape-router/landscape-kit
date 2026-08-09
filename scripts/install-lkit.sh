#!/bin/sh
set -eu

RELEASE_BASE_URL=https://github.com/landscape-router/landscape-kit/releases/latest/download
install_path=${LKIT_INSTALL_PATH:-/usr/local/bin/lkit}
work_directory=
staged_path=

die() {
    printf 'install-lkit: %s\n' "$*" >&2
    exit 1
}

cleanup() {
    if [ -n "$staged_path" ]; then
        rm -f -- "$staged_path"
    fi
    if [ -n "$work_directory" ]; then
        rm -rf -- "$work_directory"
    fi
}

trap cleanup 0 1 2 15

run_landscape_install=false
if [ "$#" -gt 0 ]; then
    if [ "$1" != install ]; then
        die "the only supported argument is 'install' followed by lkit install options"
    fi
    run_landscape_install=true
    shift
fi

[ "$(id -u)" -eq 0 ] || die "run this installer as root, for example through sudo"
[ "$(uname -s)" = Linux ] || die "only Linux is supported"

if command -v ldd >/dev/null 2>&1; then
    libc_description=$(ldd --version 2>&1 || :)
    case "$libc_description" in
        *musl*)
            die "musl-based distributions (including Alpine) are not supported by the current glibc release binaries"
            ;;
    esac
fi

case "$(uname -m)" in
    x86_64 | amd64)
        asset_name=lkit-x86_64
        ;;
    aarch64 | arm64)
        asset_name=lkit-aarch64
        ;;
    *)
        die "unsupported architecture: $(uname -m)"
        ;;
esac

for command_name in awk dirname id install mktemp mv rm sha256sum uname; do
    command -v "$command_name" >/dev/null 2>&1 || die "$command_name is required"
done

downloader=
if command -v curl >/dev/null 2>&1; then
    downloader=curl
elif command -v wget >/dev/null 2>&1; then
    downloader=wget
else
    die "curl or wget is required"
fi

case "$install_path" in
    /*) ;;
    *) die "LKIT_INSTALL_PATH must be an absolute path" ;;
esac

install_directory=$(dirname "$install_path")
[ -d "$install_directory" ] || die "install directory does not exist: $install_directory"
[ ! -d "$install_path" ] || die "install path is a directory: $install_path"

work_directory=$(mktemp -d "${TMPDIR:-/tmp}/lkit-install.XXXXXX")

download() {
    asset=$1
    output=$2
    url="$RELEASE_BASE_URL/$asset"
    case "$downloader" in
        curl)
            curl \
                --proto '=https' \
                --proto-redir '=https' \
                --tlsv1.2 \
                --fail \
                --silent \
                --show-error \
                --location \
                --output "$output" \
                "$url"
            ;;
        wget)
            wget \
                --https-only \
                --secure-protocol=TLSv1_2 \
                --no-verbose \
                --output-document "$output" \
                "$url"
            ;;
    esac
}

download "$asset_name" "$work_directory/$asset_name"
download SHA256SUMS "$work_directory/SHA256SUMS"

if ! awk -v asset="$asset_name" '
    $2 == asset && length($1) == 64 && $1 !~ /[^0-9a-f]/ {
        print $1 "  " asset
        matches++
    }
    END { if (matches != 1) exit 1 }
' "$work_directory/SHA256SUMS" >"$work_directory/selected.sha256"; then
    die "SHA256SUMS does not contain exactly one valid entry for $asset_name"
fi

if ! (cd "$work_directory" && sha256sum --check --strict selected.sha256); then
    die "SHA-256 verification failed for $asset_name"
fi

staged_path=$(mktemp "$install_directory/.lkit.XXXXXX")
install -m 0755 "$work_directory/$asset_name" "$staged_path"
version_output=$("$staged_path" --version) || die "downloaded lkit failed its version check"
case "$version_output" in
    'lkit '*) ;;
    *) die "downloaded lkit returned an invalid version string" ;;
esac

mv -f -- "$staged_path" "$install_path"
staged_path=
cleanup
trap - 0 1 2 15

printf 'install-lkit: installed %s at %s\n' "$version_output" "$install_path"

if [ "$run_landscape_install" = true ]; then
    exec "$install_path" install "$@"
fi
