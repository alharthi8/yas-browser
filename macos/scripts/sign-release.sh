#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/../.."
umask 077
key="$(mktemp)"
trap 'rm -f "$key"' EXIT
python3 - "$key" <<'PY'
import os,base64,sys
key=base64.b64decode(os.environ['YAS_UPDATE_SIGNING_KEY'],validate=True)
assert len(key)==64
open(sys.argv[1],'wb').write(key)
PY
version="$(python3 -c 'import tomllib;print(tomllib.load(open("macos/Cargo.toml","rb"))["package"]["version"])')"
preview="macos/dist/Yas-Browser-$version-macOS-Apple-Silicon-preview.zip"
asset="macos/dist/yas-browser-ota-$version-aarch64-apple-darwin.zip"
cp "$preview" "$asset"
zipsign sign zip "$asset" "$key"
zipsign verify zip "$asset" release.pub
"macos/dist/Yas Browser.app/Contents/MacOS/yas-macos" --yas-verify-update "$asset"
cp "$asset" "$preview"
export YAS_SIGNED_UPDATE_FIXTURE="$PWD/$asset"
cargo test --release --locked --manifest-path "macos/Cargo.toml" signed_bundle_install_and_tamper_rejection -- --ignored --nocapture
(cd macos/dist && shasum -a 256 *.zip > SHA256SUMS.txt)
