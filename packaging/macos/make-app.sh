#!/bin/sh
# Builds Coppersmith.app in target/. Run on a Mac with Rust and the Xcode
# command line tools. Pass --universal for one app that runs on both Apple
# Silicon and Intel Macs (needs: rustup target add aarch64-apple-darwin x86_64-apple-darwin).
set -e
cd "$(dirname "$0")/../.."
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

if [ "$1" = "--universal" ]; then
    cargo build --release --target aarch64-apple-darwin
    cargo build --release --target x86_64-apple-darwin
    mkdir -p target/universal
    lipo -create -output target/universal/coppersmith \
        target/aarch64-apple-darwin/release/coppersmith \
        target/x86_64-apple-darwin/release/coppersmith
    binary=target/universal/coppersmith
else
    cargo build --release
    binary=target/release/coppersmith
fi

app=target/Coppersmith.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/coppersmith"

# The icon, from the same pictures the other platforms use.
iconset=target/coppersmith.iconset
rm -rf "$iconset"
mkdir -p "$iconset"
for s in 16 32 128 256 512; do
    sips -z $s $s assets/icons/512.png --out "$iconset/icon_${s}x${s}.png" >/dev/null
    d=$((s * 2))
    sips -z $d $d assets/icons/512.png --out "$iconset/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/coppersmith.icns"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Coppersmith</string>
    <key>CFBundleDisplayName</key><string>Coppersmith</string>
    <key>CFBundleIdentifier</key><string>app.coppersmith</string>
    <key>CFBundleExecutable</key><string>coppersmith</string>
    <key>CFBundleIconFile</key><string>coppersmith</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>CFBundleVersion</key><string>$version</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
</dict>
</plist>
PLIST

echo "Made $app. Drag it to Applications."
