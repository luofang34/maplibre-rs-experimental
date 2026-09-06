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
# The host orders its copies and presentation after the map's draws by committing them on
# the map's own Metal queue, which wgpu 22 keeps private at every layer.
if ! grep -q "pub fn raw_handle(&self) -> metal::CommandQueue" "$hal_metal"; then
  cat >> "$hal_metal" <<'ACCESSOR'

impl Queue {
    /// The Metal command queue, for hosts that order their own work after the map's.
    pub fn raw_handle(&self) -> metal::CommandQueue {
        self.raw.lock().clone()
    }
}
ACCESSOR
fi
core_resource="$here/vendor/wgpu-core-22.1.0/src/resource.rs"
if ! grep -q "pub unsafe fn queue_as_hal" "$core_resource"; then
  cat >> "$core_resource" <<'ACCESSOR'

impl Global {
    /// # Safety
    ///
    /// - The raw queue handle must not be manually destroyed
    pub unsafe fn queue_as_hal<A: HalApi, F: FnOnce(Option<&A::Queue>) -> R, R>(
        &self,
        id: crate::id::QueueId,
        hal_queue_callback: F,
    ) -> R {
        let hub = A::hub(self);
        let queue = hub.queues.get(id).ok();
        let hal_queue = queue.as_ref().and_then(|queue| queue.raw.as_ref());
        hal_queue_callback(hal_queue)
    }
}
ACCESSOR
fi
wgpu_backend="$here/vendor/wgpu-22.1.0/src/backend/wgpu_core.rs"
if ! grep -q "pub unsafe fn queue_as_hal" "$wgpu_backend"; then
  cat >> "$wgpu_backend" <<'ACCESSOR'

impl ContextWgpuCore {
    pub unsafe fn queue_as_hal<A: wgc::hal_api::HalApi, F: FnOnce(Option<&A::Queue>) -> R, R>(
        &self,
        queue: &Queue,
        hal_queue_callback: F,
    ) -> R {
        unsafe { self.0.queue_as_hal::<A, F, R>(queue.id, hal_queue_callback) }
    }
}
ACCESSOR
fi
wgpu_lib="$here/vendor/wgpu-22.1.0/src/lib.rs"
if ! grep -q "hal_queue_callback" "$wgpu_lib"; then
  cat >> "$wgpu_lib" <<'ACCESSOR'

impl Queue {
    /// Returns the inner hal Queue using a callback. The hal queue will be `None` if the
    /// backend type argument does not match with this wgpu Queue
    ///
    /// # Safety
    ///
    /// - The raw handle passed to the callback must not be manually destroyed.
    #[cfg(wgpu_core)]
    pub unsafe fn as_hal<A: wgc::hal_api::HalApi, F: FnOnce(Option<&A::Queue>) -> R, R>(
        &self,
        hal_queue_callback: F,
    ) -> Option<R> {
        self.context
            .as_any()
            .downcast_ref::<crate::backend::ContextWgpuCore>()
            .map(|ctx| unsafe {
                ctx.queue_as_hal::<A, F, R>(
                    self.data.as_ref().downcast_ref().unwrap(),
                    hal_queue_callback,
                )
            })
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
