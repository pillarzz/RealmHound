#!/bin/sh
set -eu

repo=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
build="$repo/realmhound/target/release"
if [ ! -x "$build/RealmHound" ] || [ ! -x "$build/realmhound-capture" ]; then
    echo "Build first: cd realmhound && cargo build --workspace --release" >&2
    exit 1
fi

prefix=${PREFIX:-/usr/local}
echo "Installing RealmHound and the restricted capture helper into $prefix/bin"
sudo install -d -m 0755 "$prefix/bin" "$prefix/share/applications" "$prefix/share/icons/hicolor/64x64/apps"
sudo install -o root -g root -m 0755 "$build/RealmHound" "$prefix/bin/RealmHound"
sudo install -o root -g root -m 0755 "$build/realmhound-capture" "$prefix/bin/realmhound-capture"
sudo setcap cap_net_raw=ep "$prefix/bin/realmhound-capture"
sudo install -o root -g root -m 0644 "$repo/packaging/linux/realmhound.desktop" "$prefix/share/applications/realmhound.desktop"
for size in 64 128 256 512; do
    icon_dir="$prefix/share/icons/hicolor/${size}x${size}/apps"
    sudo install -d -m 0755 "$icon_dir"
    sudo install -o root -g root -m 0644 \
        "$repo/packaging/linux/icons/hicolor/${size}x${size}/apps/realmhound.png" \
        "$icon_dir/realmhound.png"
done
# Refresh the shell caches so the launcher and icon appear without a re-login.
if command -v update-desktop-database >/dev/null 2>&1; then
    sudo update-desktop-database -q "$prefix/share/applications" || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1 && [ -f "$prefix/share/icons/hicolor/index.theme" ]; then
    sudo gtk-update-icon-cache -q -t -f "$prefix/share/icons/hicolor" || true
fi
echo "Installed. Launch RealmHound normally from your desktop or run $prefix/bin/RealmHound."
echo "Verify helper capability: /usr/sbin/getcap $prefix/bin/realmhound-capture"
