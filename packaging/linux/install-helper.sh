#!/bin/sh
set -eu

# Ignore the caller's PATH: privileged lookups (sudo, setcap, package tools)
# must not resolve through a user-controlled directory.
PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
export PATH

# Manual, privileged capture-helper install or upgrade.
#
# This is the rare administrative step. The GUI never runs it and never
# escalates. Only the helper is touched: the GUI and its launcher stay
# user-owned.
#
# Obtain the helper from an official release and pass the SHA-256 published
# with that release. The destination is fixed; the helper is installed
# root:root 0755 with exactly cap_net_raw=ep.

usage() {
    echo "Usage: sudo $0 --sha256 <64-hex-digest> <path-to-realmhound-capture>" >&2
    echo "" >&2
    echo "The digest is the official 'RealmHound-linux-x86_64-helper.sha256' value." >&2
    exit 2
}

if [ "$(id -u)" -ne 0 ]; then
    echo "Run this helper installer with sudo." >&2
    exit 1
fi

expected=""
src=""
while [ $# -gt 0 ]; do
    case "$1" in
        --sha256)
            [ $# -ge 2 ] || usage
            expected=$2
            shift 2
            ;;
        -h|--help)
            usage
            ;;
        -*)
            usage
            ;;
        *)
            [ -z "$src" ] || usage
            src=$1
            shift
            ;;
    esac
done

[ -n "$expected" ] && [ -n "$src" ] || usage
case "$expected" in
    *[!0-9a-fA-F]*|'')
        echo "The SHA-256 must be 64 hexadecimal characters." >&2
        exit 2
        ;;
esac
[ "${#expected}" -eq 64 ] || {
    echo "The SHA-256 must be 64 hexadecimal characters." >&2
    exit 2
}

[ -f "$src" ] || {
    echo "$src is not a regular file." >&2
    exit 1
}
[ ! -L "$src" ] || {
    echo "$src is a symlink; pass the downloaded regular file." >&2
    exit 1
}
if [ "$(stat -c '%s' "$src")" -gt 104857600 ]; then
    echo "$src exceeds the 100 MB helper limit." >&2
    exit 1
fi

getcap_bin=$(command -v getcap 2>/dev/null || true)
if [ -z "$getcap_bin" ] && [ -x /usr/sbin/getcap ]; then
    getcap_bin=/usr/sbin/getcap
fi
setcap_bin=$(command -v setcap 2>/dev/null || true)
if [ -z "$setcap_bin" ] && [ -x /usr/sbin/setcap ]; then
    setcap_bin=/usr/sbin/setcap
fi
if [ -z "$setcap_bin" ] || [ -z "$getcap_bin" ]; then
    echo "setcap/getcap not found. Install libcap2-bin (Debian/Ubuntu) or libcap (Arch)." >&2
    exit 1
fi

helper_dir="/usr/local/libexec/realmhound"
helper_dest="$helper_dir/realmhound-capture"

install -d -m 0755 -o root -g root "$helper_dir"
if [ -L "$helper_dir" ]; then
    echo "$helper_dir is a symlink; refusing to install through it." >&2
    exit 1
fi
if [ -d "$helper_dest" ]; then
    echo "$helper_dest is a directory; refusing to replace it." >&2
    exit 1
fi
if [ -e "$helper_dest" ] && [ -L "$helper_dest" ]; then
    echo "$helper_dest is a symlink; refusing to replace it." >&2
    exit 1
fi
chown root:root "$helper_dir"
chmod 0755 "$helper_dir"
helper_mode=$(stat -c '%a' "$helper_dir")
case "$helper_mode" in
    *[2367])
        echo "$helper_dir is writable by group or other ($helper_mode); refusing." >&2
        exit 1
        ;;
esac

# Copy into a private root-owned inode in the destination directory, verify
# the copy rather than the caller's file (closing the hash/copy race), then
# rename it into place.
tmp=$(mktemp "$helper_dir/.realmhound-capture.XXXXXX")
trap 'rm -f "$tmp"' EXIT
cp -- "$src" "$tmp"
chown root:root "$tmp"
chmod 0755 "$tmp"

actual=$(sha256sum -- "$tmp" | awk '{print $1}')
if [ "$actual" != "$(printf '%s' "$expected" | tr 'A-F' 'a-f')" ]; then
    echo "SHA-256 mismatch: expected $expected, got $actual" >&2
    echo "The helper was not installed." >&2
    exit 1
fi

if ! timeout 10 "$tmp" --version >/dev/null 2>&1; then
    echo "The supplied helper cannot run on this system; nothing was installed." >&2
    exit 1
fi

"$setcap_bin" cap_net_raw=ep "$tmp"
tmp_cap=$("$getcap_bin" "$tmp" 2>/dev/null || true)
case "$tmp_cap" in
    *"cap_net_raw=ep"*) ;;
    *)
        echo "Failed to apply cap_net_raw=ep (got: $tmp_cap)" >&2
        exit 1
        ;;
esac

mv -f "$tmp" "$helper_dest"
trap - EXIT

final_cap=$("$getcap_bin" "$helper_dest" 2>/dev/null || true)
case "$final_cap" in
    *"cap_net_raw=ep"*) ;;
    *)
        echo "The helper was installed but lost cap_net_raw=ep (got: $final_cap)" >&2
        exit 1
        ;;
esac

echo "Installed $helper_dest"
echo "$final_cap"
echo "The RealmHound GUI was not touched."
