#!/bin/sh
set -eu

# Manual, privileged capture-helper install or upgrade.
#
# This is the rare administrative step. The GUI never runs it and never
# escalates. Only the helper is touched: the GUI and its launcher stay
# user-owned.
#
# Obtain the helper from an official release and pass the SHA-256 published
# with that release. The destination is fixed; the helper keeps an existing
# root-owned installation's execution group and restrictive mode, or uses
# root:root 0755 for a fresh installation.

# Ignore the caller's PATH: privileged lookups (setcap, package tools) must not
# resolve through a user-controlled directory.
PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
export PATH

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
legacy_helper="/usr/local/bin/realmhound-capture"

# Directory writability is not proof of ownership; ask the package manager.
pkg_owned() {
    path=$1
    if command -v dpkg-query >/dev/null 2>&1 && dpkg-query -S "$path" >/dev/null 2>&1; then
        return 0
    fi
    if command -v rpm >/dev/null 2>&1 && rpm -qf "$path" >/dev/null 2>&1; then
        return 0
    fi
    if command -v pacman >/dev/null 2>&1 && pacman -Qo "$path" >/dev/null 2>&1; then
        return 0
    fi
    return 1
}

# Print a mode without setuid/setgid/sticky or group/other write, or fail.
safe_mode() {
    mode=$1
    path=$2
    case "$mode" in
        [0-7]) mode="00$mode" ;;
        [0-7][0-7]) mode="0$mode" ;;
    esac
    case "$mode" in
        [0-7][0-7][0-7]) ;;
        [0-7][0-7][0-7][0-7])
            case "$mode" in
                0*) mode=${mode#0} ;;
                *)
                    echo "$path has special permission bits ($mode); refusing." >&2
                    return 1
                    ;;
            esac
            ;;
        *)
            echo "$path has an unsafe mode ($mode); refusing." >&2
            return 1
            ;;
    esac
    case "$mode" in
        ?[2367]?|??[2367])
            echo "$path is writable by group or other ($mode); refusing." >&2
            return 1
            ;;
    esac
    printf '%s\n' "$mode"
}

# Every directory from /usr/local down to the helper must be a real root-owned
# directory that unprivileged users cannot redirect or modify.
require_protected_dir() {
    path=$1
    if [ ! -d "$path" ] || [ -L "$path" ]; then
        echo "$path must be a real directory, not a symlink." >&2
        exit 1
    fi
    if [ "$(stat -c '%u' "$path")" -ne 0 ]; then
        echo "$path is not root-owned." >&2
        exit 1
    fi
    safe_mode "$(stat -c '%a' "$path")" "$path" >/dev/null
}

# Preserve an existing root-owned helper's execution group and restrictive
# mode. Fresh installations use the documented world-executable policy.
helper_group=root
helper_mode=755
existing_helper=""
for candidate in "$helper_dest" "$legacy_helper"; do
    if [ -f "$candidate" ] && [ ! -L "$candidate" ]; then
        existing_helper=$candidate
        break
    fi
done
if [ -n "$existing_helper" ]; then
    if [ "$(stat -c '%U' "$existing_helper")" != root ]; then
        echo "$existing_helper is not root-owned; refusing to replace a user-controlled helper." >&2
        exit 1
    fi
    helper_mode=$(safe_mode "$(stat -c '%a' "$existing_helper")" "$existing_helper")
    helper_group=$(stat -c '%G' "$existing_helper")
fi

require_protected_dir /
require_protected_dir /usr
# Create a missing directory beneath a validated parent without changing an
# existing directory's mode. Existing restrictive modes are preserved.
ensure_protected_dir() {
    path=$1
    if [ -e "$path" ] || [ -L "$path" ]; then
        require_protected_dir "$path"
        return
    fi
    parent=$(dirname -- "$path")
    require_protected_dir "$parent"
    install -d -m 0755 -o root -g root "$path"
    require_protected_dir "$path"
}

require_protected_dir /
require_protected_dir /usr
ensure_protected_dir /usr/local
ensure_protected_dir /usr/local/libexec
ensure_protected_dir "$helper_dir"

if [ -d "$helper_dest" ]; then
    echo "$helper_dest is a directory; refusing to replace it." >&2
    exit 1
fi
if [ -e "$helper_dest" ] && [ -L "$helper_dest" ]; then
    echo "$helper_dest is a symlink; refusing to replace it." >&2
    exit 1
fi
if [ -e "$helper_dest" ] && pkg_owned "$helper_dest"; then
    echo "$helper_dest is owned by a package manager; refusing to overwrite it." >&2
    exit 1
fi

# Copy into a private root-owned inode in the destination directory, verify
# the copy rather than the caller's file (closing the hash/copy race), then
# rename it into place.
tmp=$(mktemp "$helper_dir/.realmhound-capture.XXXXXX")
trap 'rm -f "$tmp"' EXIT
cp -- "$src" "$tmp"
chown root:root "$tmp"
chmod 0700 "$tmp"

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

chown "root:$helper_group" "$tmp"
chmod "$helper_mode" "$tmp"
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
if [ "$(stat -c '%U' "$helper_dest")" != root ] \
    || [ "$(stat -c '%G' "$helper_dest")" != "$helper_group" ] \
    || [ "$(stat -c '%a' "$helper_dest")" -ne "$helper_mode" ]; then
    echo "The helper's ownership or permissions were not preserved." >&2
    exit 1
fi

echo "Installed $helper_dest"
echo "$final_cap"
echo "The RealmHound GUI was not touched."
