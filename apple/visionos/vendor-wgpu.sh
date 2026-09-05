#!/bin/sh
# Copies wgpu 22, naga, metal-rs and the Objective-C runtime crates from the cargo registry into vendor/ and widens their Apple
# cfgs to visionOS, which they gate on macOS and iOS only. The copies are otherwise
# unchanged; .cargo/config.toml patches crates.io with them for this crate alone.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
registry=$(ls -d "$HOME"/.cargo/registry/src/*/ | head -1)
mkdir -p "$here/vendor"
for crate in wgpu-22.1.0 wgpu-core-22.1.0 wgpu-hal-22.0.0 naga-22.1.0 metal-0.29.0 objc-0.2.7 block-0.1.6; do
  if [ ! -d "$registry/$crate" ]; then
    echo "missing $registry/$crate; run cargo fetch in the main workspace first" >&2
    exit 1
  fi
  rm -rf "$here/vendor/$crate"
  cp -R "$registry/$crate" "$here/vendor/$crate"
  rm -f "$here/vendor/$crate/.cargo-ok"
  find "$here/vendor/$crate" \( -name '*.rs' -o -name Cargo.toml \) -type f | while read -r file; do
    sed -i '' \
      -e 's/target_os = "ios"/any(target_os = "ios", target_os = "visionos")/g' \
      -e 's/target_os = \\"ios\\"/any(target_os = \\"ios\\", target_os = \\"visionos\\")/g' \
      -e 's/target_os=\\"ios\\"/any(target_os=\\"ios\\", target_os=\\"visionos\\")/g' \
      "$file"
  done
done
# The host copies frames out of the map's texture, which needs the Metal object wgpu-hal 22
# keeps private.
hal_metal="$here/vendor/wgpu-hal-22.0.0/src/metal/mod.rs"
if ! grep -q "pub fn raw_handle" "$hal_metal"; then
  cat >> "$hal_metal" <<'ACCESSOR'

impl Texture {
    /// The Metal texture, for hosts that copy frames out of it.
    pub fn raw_handle(&self) -> &metal::Texture {
        &self.raw
    }
}
ACCESSOR
fi
# Path dependencies are not lint-capped the way registry crates are; block 0.1 trips a
# future-incompatibility lint that is only a warning from the registry.
block_lib="$here/vendor/block-0.1.6/src/lib.rs"
if ! grep -q "allow(uninhabited_static)" "$block_lib"; then
  printf '#![allow(uninhabited_static)]\n%s' "$(cat "$block_lib")" > "$block_lib"
fi
echo "vendored into $here/vendor"
