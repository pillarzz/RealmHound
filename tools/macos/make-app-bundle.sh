#!/usr/bin/env bash
#
# Assemble RealmHound.app around an already-built macOS binary.
#
# A bare Mach-O has nowhere to carry an icon, so Finder shows it as a generic
# executable and double-clicking it opens Terminal. Wrapping the very same
# universal binary in a bundle gives it the RealmHound icon, a real Dock entry,
# and a plain double-click launch.
#
# The self-updater is unaffected: it rewrites `current_exe()`, which inside a
# bundle is Contents/MacOS/RealmHound, leaving Info.plist and the icon in place.
#
# Usage: tools/macos/make-app-bundle.sh <binary> <output-directory>

set -euo pipefail

if [[ $# -ne 2 ]]; then
    echo "usage: $0 <path/to/RealmHound-binary> <output-directory>" >&2
    exit 2
fi

binary=$1
out_dir=$2
repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
icon="$repo_root/realmhound/crates/realmhound/assets/icon.icns"
workspace_manifest="$repo_root/realmhound/Cargo.toml"

for required in "$binary" "$icon" "$workspace_manifest"; do
    if [[ ! -f $required ]]; then
        echo "missing required input: $required" >&2
        exit 1
    fi
done

version=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$workspace_manifest" | head -n 1)
if [[ -z $version ]]; then
    echo "could not read the workspace version from $workspace_manifest" >&2
    exit 1
fi

app="$out_dir/RealmHound.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

cp "$icon" "$app/Contents/Resources/RealmHound.icns"

cat >"$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>CFBundleDisplayName</key>
    <string>RealmHound</string>
    <key>CFBundleExecutable</key>
    <string>RealmHound</string>
    <key>CFBundleIconFile</key>
    <string>RealmHound</string>
    <key>CFBundleIdentifier</key>
    <string>io.github.pillarzz.realmhound</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>RealmHound</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>$version</string>
    <key>CFBundleVersion</key>
    <string>$version</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.utilities</string>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
PLIST

cp "$binary" "$app/Contents/MacOS/RealmHound"
chmod +x "$app/Contents/MacOS/RealmHound"

# Signing the bundle has to come last: it covers Info.plist and the icon too.
# Ad-hoc only, so arm64 will execute it; this is not notarization.
codesign --sign - --force --timestamp=none "$app"
codesign --verify --verbose "$app"

echo "built $app (version $version)"
