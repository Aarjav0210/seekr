#!/bin/sh
# Build the archive install.sh downloads: dist/seekr-macos-universal.tar.gz,
# holding seekr/seekr (one binary for Apple silicon and Intel), the launcher
# files from contrib/, LICENSE and the docs, plus its .sha256. Run by
# .github/workflows/release.yml for every v* tag; locally it needs both targets:
#
#   rustup target add aarch64-apple-darwin x86_64-apple-darwin
#   contrib/package.sh
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
NAME=seekr-macos-universal.tar.gz
# Rust's own floor for Apple silicon; the bundled SQLite is compiled for it too.
export MACOSX_DEPLOYMENT_TARGET=${MACOSX_DEPLOYMENT_TARGET:-11.0}

for t in aarch64-apple-darwin x86_64-apple-darwin; do
  cargo build --release --locked -p seekr --target "$t"
done

STAGE=target/package/seekr
rm -rf target/package dist
mkdir -p "$STAGE/contrib/hammerspoon" "$STAGE/contrib/kitty" dist
lipo -create -output "$STAGE/seekr" \
  target/aarch64-apple-darwin/release/seekr target/x86_64-apple-darwin/release/seekr
# Apple silicon runs only signed code; an ad-hoc signature is enough for a
# binary that curl downloaded (no quarantine flag, so Gatekeeper never asks).
codesign --force --sign - "$STAGE/seekr"
cp contrib/install-launcher.sh "$STAGE/contrib/"
cp contrib/hammerspoon/seekr.lua "$STAGE/contrib/hammerspoon/"
cp contrib/kitty/seekr.conf "$STAGE/contrib/kitty/"
cp LICENSE README.md KEYBOARD_SHORTCUTS.md "$STAGE/"

# COPYFILE_DISABLE keeps macOS's ._ metadata files out of the archive.
COPYFILE_DISABLE=1 tar -czf "dist/$NAME" -C target/package seekr
(cd dist && shasum -a 256 "$NAME" > "$NAME.sha256")
echo "dist/$NAME ($(lipo -archs "$STAGE/seekr"), $("$STAGE/seekr" --version))"
