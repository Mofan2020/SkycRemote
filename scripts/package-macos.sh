#!/usr/bin/env bash
#
# Build the macOS program: the application bundle and the disk image
# that holds it.
#
# What comes out:
#
#   target/dist/SkycRemote.app            the program, to be dragged
#                                        into "Applications"
#   target/dist/SkycRemote-<version>.dmg   the disk image that is
#                                        handed to people
#
# The bundle carries two programs: the window, and the command line
# tool.  The tool is put on the PATH when the program starts, so the
# two are installed together.
#
# Usage: scripts/package-macos.sh

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

name="SkycRemote"
bundle_id="com.skyc8266.skycremote"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' crates/skycremote-app/Cargo.toml | head -1)"
out="$root/target/dist"
app="$out/$name.app"

export CARGO_TERM_COLOR=always
export CARGO_INCREMENTAL=0

if [ -z "$version" ]; then
  echo "the version could not be read" >&2
  exit 1
fi

echo "building $name $version"

cargo build --release --bin skycremote --bin skycremote-cli

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

cp target/release/skycremote "$app/Contents/MacOS/$name"
cp target/release/skycremote-cli "$app/Contents/MacOS/skycremote-cli"

# ---- the icon ----
#
# The icon is drawn by scripts/make-icon.py and turned into the shape
# macOS wants by iconutil.
iconset="$out/AppIcon.iconset"
rm -rf "$iconset"
mkdir -p "$iconset"
for size in 16 32 64 128 256 512 1024; do
  python3 scripts/make-icon.py "$iconset/icon-${size}x${size}.png" "$size" > /dev/null
done
cp "$iconset/icon-32x32.png" "$iconset/icon-16x16@2x.png"
cp "$iconset/icon-64x64.png" "$iconset/icon-32x32@2x.png"
cp "$iconset/icon-256x256.png" "$iconset/icon-128x128@2x.png"
cp "$iconset/icon-512x512.png" "$iconset/icon-256x256@2x.png"
cp "$iconset/icon-1024x1024.png" "$iconset/icon-512x512@2x.png"
iconutil --convert icns "$iconset" --output "$app/Contents/Resources/AppIcon.icns"
rm -rf "$iconset"

# ---- the description of the bundle ----
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>$name</string>
    <key>CFBundleDisplayName</key>
    <string>$name</string>
    <key>CFBundleIdentifier</key>
    <string>$bundle_id</string>
    <key>CFBundleExecutable</key>
    <string>$name</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>$version</string>
    <key>CFBundleVersion</key>
    <string>$version</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.utilities</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSSupportsAutomaticGraphicsSwitching</key>
    <true/>
</dict>
</plist>
PLIST

# ---- the signature ----
#
# The signature is not one Apple hands out; it is a local one.  It is
# enough for the program to run on the machine that made it, and it
# keeps the parts of the bundle together so that macOS does not
# rewrite the program behind the person's back.
codesign --force --sign - --timestamp=none "$app" > /dev/null 2>&1 || \
  echo "the bundle could not be signed; it will still run here" >&2

# ---- the disk image ----
stage="$out/dmg"
rm -rf "$stage"
mkdir -p "$stage"
cp -R "$app" "$stage/"
ln -s /Applications "$stage/Applications"

dmg="$out/$name-$version.dmg"
rm -f "$dmg"
hdiutil create -volname "$name" -srcfolder "$stage" -ov -format UDZO "$dmg" > /dev/null
rm -rf "$stage"

echo
echo "done:"
echo "  $app"
echo "  $dmg"
