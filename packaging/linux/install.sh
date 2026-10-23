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
sudo install -o root -g root -m 0644 "$repo/packaging/linux/realmhound.png" "$prefix/share/icons/hicolor/64x64/apps/realmhound.png"
echo "Installed. Launch RealmHound normally from your desktop or run $prefix/bin/RealmHound."
echo "Verify helper capability: /usr/sbin/getcap $prefix/bin/realmhound-capture"
