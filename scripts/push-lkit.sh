#!/bin/sh
set -eu

host=${1:?expected an SSH host or user@host}
binary=${CARGO_TARGET_DIR:-target}/release/lkit
if [ ! -f "$binary" ]; then
    echo "Missing $binary. Run 'just build' first." >&2
    exit 1
fi
checksum=$(sha256sum < "$binary")
checksum=${checksum%% *}

ssh -T -- "$host" "set -eu
expected=$checksum
"'directory=/usr/local/bin
staged=$(mktemp "$directory/.lkit.XXXXXX")
cleanup() { rm -f -- "$staged"; }
trap cleanup 0
cat > "$staged"
printf "%s  %s\n" "$expected" "$staged" | sha256sum -c - >/dev/null
chmod 0755 "$staged"
"$staged" --version
mv -fT -- "$staged" "$directory/lkit"
' < "$binary"
