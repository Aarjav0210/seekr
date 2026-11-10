#!/bin/sh
# Build the archive install.sh downloads: dist/seekr-macos-universal.tar.gz,
# holding seekr/seekr.app (one binary for Apple silicon and Intel, which is
# both the launcher app and the `seekr` command), LICENSE and the docs, plus
# its .sha256. Run by .github/workflows/release.yml for every v* tag; locally
# it needs both targets:
#
#   rustup target add aarch64-apple-darwin x86_64-apple-darwin
#   contrib/package.sh
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
NAME=seekr-macos-universal.tar.gz
# Rust's own floor for Apple silicon; the bundled SQLite is compiled for it too.
export MACOSX_DEPLOYMENT_TARGET=${MACOSX_DEPLOYMENT_TARGET:-11.0}
VERSION=$(sed -n 's/^version = "\(.*\)"$/\1/p' crates/seekr/Cargo.toml | head -n 1)

for t in aarch64-apple-darwin x86_64-apple-darwin; do
  cargo build --release --locked -p seekr --target "$t"
done

STAGE=target/package/seekr
APP="$STAGE/seekr.app"
rm -rf target/package dist
mkdir -p "$APP/Contents/MacOS" dist
lipo -create -output "$APP/Contents/MacOS/seekr" \
  target/aarch64-apple-darwin/release/seekr target/x86_64-apple-darwin/release/seekr
sed "s/@VERSION@/$VERSION/g" contrib/macos/Info.plist > "$APP/Contents/Info.plist"
plutil -lint -s "$APP/Contents/Info.plist"
# Apple silicon runs only signed code; an ad-hoc signature is enough for an
# app that curl downloaded (no quarantine flag, so Gatekeeper never asks).
codesign --force --sign - --identifier io.github.aarjav0210.seekr "$APP"
codesign --verify --strict "$APP"
cp LICENSE README.md KEYBOARD_SHORTCUTS.md "$STAGE/"

# COPYFILE_DISABLE keeps macOS's ._ metadata files out of the archive.
COPYFILE_DISABLE=1 tar -czf "dist/$NAME" -C target/package seekr
(cd dist && shasum -a 256 "$NAME" > "$NAME.sha256")
echo "dist/$NAME ($(lipo -archs "$APP/Contents/MacOS/seekr"), $("$APP/Contents/MacOS/seekr" --version))"
