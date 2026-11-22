#!/bin/sh
set -eu

# Official Linux installer.
#
# Runs as the logged-in user: the GUI and its launcher are user-owned and
# replace themselves without elevation. sudo is used only for the protected
# capture helper and for retiring the previous system-wide installation.
#
# The previous launch path is not changed until the helper has been installed
# and verified, and package-owned files are never removed.

# Ignore the caller's PATH: privileged lookups (sudo, setcap, package tools)
# must not resolve through a user-controlled directory.
PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
export PATH

repo=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
build="$repo/realmhound/target/release"
if [ ! -x "$build/RealmHound" ] || [ ! -x "$build/realmhound-capture" ]; then
    echo "Build first: cd realmhound && cargo build --workspace --release" >&2
    exit 1
fi

if [ "$(id -u)" -eq 0 ]; then
    echo "Run this installer as your normal user, not with sudo." >&2
    echo "It asks for sudo only for the capture helper and legacy cleanup." >&2
    exit 1
fi

if ! command -v sudo >/dev/null 2>&1; then
    echo "sudo is required to install the protected capture helper." >&2
    exit 1
fi

if command -v pgrep >/dev/null 2>&1 && pgrep -x RealmHound >/dev/null 2>&1; then
    echo "RealmHound is running. Close it before launching the migrated version." >&2
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

gui_dir="$HOME/.local/opt/realmhound"
launcher_dir="$HOME/.local/bin"
launcher="$launcher_dir/realmhound"
data_home="${XDG_DATA_HOME:-$HOME/.local/share}"
applications_dir="$data_home/applications"
icons_root="$data_home/icons/hicolor"
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
    sudo install -d -m 0755 -o root -g root "$path"
    require_protected_dir "$path"
}

legacy_changed=0
remove_legacy() {
    path=$1
    [ -e "$path" ] || return 0
    if [ -d "$path" ] && [ ! -L "$path" ]; then
        echo "Warning: $path is a directory; leaving it in place." >&2
        return 0
    fi
    if pkg_owned "$path"; then
        echo "Leaving package-owned $path in place."
        return 0
    fi
    sudo rm -f -- "$path"
    legacy_changed=1
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

max_bytes=104857600
gui_size=$(stat -c '%s' "$build/RealmHound")
helper_size=$(stat -c '%s' "$build/realmhound-capture")
if [ "$gui_size" -gt "$max_bytes" ] || [ "$helper_size" -gt "$max_bytes" ]; then
    echo "Refusing to install: staged binary exceeds 100 MB." >&2
    exit 1
fi

# ── Stage the user GUI and launch assets; publish nothing yet ────────────────
mkdir -p "$gui_dir" "$launcher_dir" "$applications_dir"
gui_tmp="$gui_dir/.RealmHound.$$"
marker_tmp="$gui_dir/.install.json.$$"
launcher_tmp="$launcher.tmp"
desktop_tmp="$applications_dir/.realmhound.desktop.$$"
helper_tmp=""
trap 'rm -f "$gui_tmp" "$marker_tmp" "$launcher_tmp" "$desktop_tmp"; if [ -n "$helper_tmp" ]; then sudo -n rm -f "$helper_tmp" 2>/dev/null || true; fi' EXIT

install -m 0755 "$build/RealmHound" "$gui_tmp"
if ! timeout 10 "$gui_tmp" --version >/dev/null 2>&1; then
    echo "The staged GUI cannot run on this system; leaving the previous installation in place." >&2
    exit 1
fi

cat > "$launcher_tmp" <<'LAUNCHER'
#!/bin/sh
# Generated by packaging/linux/install.sh. Do not edit.
dir="${REALMHOUND_GUI_DIR:-$HOME/.local/opt/realmhound}"
exe="$dir/RealmHound"
attempt="$dir/.launch-attempt"
confirmed="$dir/startup-confirmed"

read_version() {
    version=""
    [ -x "$exe" ] || return 0
    output=$(timeout 10 "$exe" --version 2>/dev/null)
    [ $? -eq 0 ] || return 0
    version=$(printf '%s\n' "$output" | awk 'NR==1 {print $NF}')
}
read_version

# A previous launch of this version never confirmed a rendered frame: restore
# the retained previous executable before trying again. This catches binaries
# that pass --version but fail window or renderer initialization.
if [ -n "$version" ] && [ -f "$attempt" ] && [ "$(cat "$attempt" 2>/dev/null)" = "$version" ]; then
    if [ -f "$confirmed" ] && [ "$(cat "$confirmed" 2>/dev/null)" = "$version" ]; then
        rm -f "$attempt"
    elif [ -x "$exe.old" ] && ! pgrep -x RealmHound >/dev/null 2>&1; then
        mv -f -- "$exe.old" "$exe"
        rm -f "$attempt"
        read_version
    fi
fi

# Recover from an interrupted swap or a replacement that cannot run. The live
# path should exist throughout an update, but older installs may not.
if [ ! -x "$exe" ]; then
    if [ -x "$exe.old" ]; then
        mv -f -- "$exe.old" "$exe"
        read_version
    fi
elif [ -z "$version" ] && [ -x "$exe.old" ]; then
    mv -f -- "$exe.old" "$exe"
    read_version
fi

# Record this attempt; the GUI replaces it only after rendering a frame.
if [ -n "$version" ]; then
    printf '%s' "$version" > "$attempt"
fi
exec "$exe" "$@"
LAUNCHER
chmod 0755 "$launcher_tmp"

sed "s|@LAUNCHER@|$launcher|g" "$repo/packaging/linux/realmhound.desktop" > "$desktop_tmp"
chmod 0644 "$desktop_tmp"

# ── Protected helper first: the old launch path stays untouched on failure ──
echo "Installing the protected capture helper (sudo required)..."
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

helper_tmp="$helper_dir/.realmhound-capture.tmp"
sudo install -o root -g root -m 0700 "$build/realmhound-capture" "$helper_tmp"
if ! sudo timeout 10 "$helper_tmp" --version >/dev/null 2>&1; then
    echo "The staged capture helper cannot run on this system; leaving the previous helper in place." >&2
    exit 1
fi
sudo chown "root:$helper_group" "$helper_tmp"
sudo chmod "$helper_mode" "$helper_tmp"
sudo "$setcap_bin" cap_net_raw=ep "$helper_tmp"
helper_cap=$("$getcap_bin" "$helper_tmp" 2>/dev/null || true)
case "$helper_cap" in
    *"cap_net_raw=ep"*) ;;
    *)
        echo "Failed to apply cap_net_raw=ep to the helper (got: $helper_cap)" >&2
        exit 1
        ;;
esac
sudo mv -f "$helper_tmp" "$helper_dest"
helper_tmp=""
helper_cap=$("$getcap_bin" "$helper_dest" 2>/dev/null || true)
case "$helper_cap" in
    *"cap_net_raw=ep"*) ;;
    *)
        echo "The helper lost cap_net_raw=ep during installation (got: $helper_cap)" >&2
        exit 1
        ;;
esac
if [ "$(stat -c '%U' "$helper_dest")" != root ] \
    || [ "$(stat -c '%G' "$helper_dest")" != "$helper_group" ] \
    || [ "$(stat -c '%a' "$helper_dest")" -ne "$helper_mode" ]; then
    echo "The helper's ownership or permissions were not preserved." >&2
    exit 1
fi

# ── Publish the user-owned launch path only after the helper succeeded ──────
mv -f "$gui_tmp" "$gui_dir/RealmHound"

printf '{"layout":"official-user","helper":"%s"}\n' "$helper_dest" > "$marker_tmp"
chmod 0644 "$marker_tmp"
mv -f "$marker_tmp" "$gui_dir/install.json"

mv -f "$launcher_tmp" "$launcher"
if [ -d "$launcher_dir/RealmHound" ] && [ ! -L "$launcher_dir/RealmHound" ]; then
    echo "$launcher_dir/RealmHound is a directory; refusing to replace it." >&2
    exit 1
fi
ln -sf "$launcher" "$launcher_dir/RealmHound"

mv -f "$desktop_tmp" "$applications_dir/realmhound.desktop"

for size in 64 128 256 512; do
    icon_dir="$icons_root/${size}x${size}/apps"
    mkdir -p "$icon_dir"
    install -m 0644 \
        "$repo/packaging/linux/icons/hicolor/${size}x${size}/apps/realmhound.png" \
        "$icon_dir/realmhound.png"
done

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database -q "$applications_dir" || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1 && [ -f "$icons_root/index.theme" ]; then
    gtk-update-icon-cache -q -t -f "$icons_root" || true
fi
rm -f "$gui_dir/.launch-attempt"
trap - EXIT

# ── Legacy migration (last: the old installation stays usable until here) ───
remove_legacy /usr/local/bin/RealmHound
remove_legacy "$legacy_helper"
remove_legacy /usr/local/share/applications/realmhound.desktop
for size in 64 128 256 512; do
    remove_legacy "/usr/local/share/icons/hicolor/${size}x${size}/apps/realmhound.png"
done
if [ "$legacy_changed" -eq 1 ]; then
    if command -v update-desktop-database >/dev/null 2>&1; then
        sudo update-desktop-database -q /usr/local/share/applications 2>/dev/null || true
    fi
    if command -v gtk-update-icon-cache >/dev/null 2>&1 && [ -f /usr/local/share/icons/hicolor/index.theme ]; then
        sudo gtk-update-icon-cache -q -t -f /usr/local/share/icons/hicolor 2>/dev/null || true
    fi
fi

echo "Installed RealmHound to $gui_dir."
echo "Launch it from your desktop menu or run: $launcher"
echo "Protected helper: $helper_dest ($helper_cap)"
case ":$PATH:" in
    *":$launcher_dir:"*) ;;
    *)
        echo "Note: $launcher_dir is not on PATH; the desktop entry still works."
        ;;
esac
echo "Routine in-app updates need no password. Helper upgrades are a rare manual step:"
echo "  sudo \"$repo/packaging/linux/install-helper.sh\" --sha256 <official hash> <verified helper>"
