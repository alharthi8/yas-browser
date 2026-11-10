#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")"
export PATH="$HOME/.cargo/bin:$PATH"
export MACOSX_DEPLOYMENT_TARGET=13.0
if [ "$(uname -m)" != arm64 ]; then echo 'This preview build currently targets Apple Silicon only.' >&2; exit 1; fi
version="$(cargo metadata --format-version 1 --no-deps | /usr/bin/python3 -c 'import sys,json; print(json.load(sys.stdin)["packages"][0]["version"])')"
cargo test --release --locked
cargo build --release --locked
target="${CARGO_TARGET_DIR:-$PWD/target}"
binary_root="$target"
if [ -n "${CARGO_BUILD_TARGET:-}" ]; then binary_root="$target/$CARGO_BUILD_TARGET"; fi
app="$PWD/dist/Yas Browser.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$PWD/dist/AppIcon.iconset"
cp "$binary_root/release/yas-macos" "$app/Contents/MacOS/yas-macos"
chmod 755 "$app/Contents/MacOS/yas-macos"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" assets/logo.png --out "dist/AppIcon.iconset/icon_${size}x${size}.png" >/dev/null
  retina=$((size * 2))
  sips -z "$retina" "$retina" assets/logo.png --out "dist/AppIcon.iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil --convert icns --output "$app/Contents/Resources/AppIcon.icns" dist/AppIcon.iconset
cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>yas-macos</string>
<key>CFBundleIdentifier</key><string>ae.appana.yasbrowser</string>
<key>CFBundleName</key><string>Yas Browser</string>
<key>CFBundleDisplayName</key><string>Yas Browser</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleVersion</key><string>0.4.7</string>
<key>CFBundleShortVersionString</key><string>0.4.7</string>
<key>CFBundleIconFile</key><string>AppIcon</string>
<key>LSMinimumSystemVersion</key><string>13.0</string>
<key>NSHighResolutionCapable</key><true/>
<key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>
<key>NSAppTransportSecurity</key><dict><key>NSAllowsArbitraryLoadsInWebContent</key><true/></dict>
</dict></plist>
PLIST
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $version" "$app/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $version" "$app/Contents/Info.plist"
codesign --force --deep --sign "${YAS_CODESIGN_IDENTITY:--}" "$app"
codesign --verify --deep --strict --verbose=2 "$app"
file "$app/Contents/MacOS/yas-macos"
ditto -c -k --sequesterRsrc --keepParent "$app" "$PWD/dist/Yas-Browser-$version-macOS-Apple-Silicon-preview.zip"
shasum -a 256 "$PWD/dist/Yas-Browser-$version-macOS-Apple-Silicon-preview.zip"
