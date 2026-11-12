#!/bin/sh
# Builds target/coppersmith-VERSION.dmg: Coppersmith.app next to a shortcut to
# Applications, so installing is one drag. Run on a Mac; needs only what
# make-app.sh needs (hdiutil comes with macOS). Passes --universal through.
set -e
cd "$(dirname "$0")/../.."
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

sh packaging/macos/make-app.sh "$@"

stage=target/dmg
dmg="target/coppersmith-$version.dmg"
rm -rf "$stage" "$dmg"
mkdir -p "$stage"
cp -R target/Coppersmith.app "$stage/"
ln -s /Applications "$stage/Applications"

hdiutil create -volname "Coppersmith $version" -srcfolder "$stage" \
    -fs HFS+ -format UDZO -imagekey zlib-level=9 -ov "$dmg" >/dev/null
rm -rf "$stage"

echo "Made $dmg. Open it and drag Coppersmith to Applications."
