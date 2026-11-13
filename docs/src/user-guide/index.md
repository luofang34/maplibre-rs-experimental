# User Guide

`maplibre` can be embedded in native desktop/mobile applications, compiled to WebAssembly for browsers, or driven offscreen in headless servers, CLI tools, and XR compositors.

## Crate Overview

| Crate | Purpose |
| --- | --- |
| `maplibre` | Core engine: style parser & expression evaluator, Tile Component System (`tcs`), `wgpu` renderer, 3D terrain, globe/Mercator projections, and `headless` offscreen / multi-view / XR APIs. |
| `maplibre-winit` | Cross-platform windowed event loop (`winit`), pointer/touch/keyboard navigation, floating HUD controls, and native Tokio + `reqwest` tile workers. |
| `web` | Browser `wasm-bindgen` bindings supporting WebGPU and WebGL2 (`web-webgl`) with dedicated Web Worker tile decoding. |
| `apple/visionos` | C ABI (`maplibre_visionos.h`) sharing a host `MTLDevice` / `MTLCommandQueue` and per-eye `MTLTexture` targets for Apple Vision Pro. |

## Desktop Quickstart (`maplibre-winit`)

Add `maplibre` and `maplibre-winit` to your binary crate and call `maplibre_winit::run_headed_map`:

```rust,no_run
use std::path::PathBuf;

use maplibre::{render::settings::WgpuSettings, style::Style};
use maplibre_winit::{run_headed_map, HeadedMapOptions, WinitMapWindowConfig};

const STYLE_URL: &str = "https://demotiles.maplibre.org/style.json";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut style: Style = maplibre::platform::run_multithreaded(async {
        reqwest::get(STYLE_URL).await?.json().await
    })?;
    style.center = Some([-74.006, 40.7128]);
    style.zoom = Some(11.0);
    for (index, layer) in style.layers.iter_mut().enumerate() {
        layer.index = index as u32 + 1;
    }

    let options = HeadedMapOptions {
        max_pitch_degrees: 75.0,
        show_navigation_controls: true,
        ..HeadedMapOptions::default()
    };

    run_headed_map(
        Some(PathBuf::from("./maplibre-cache")),
        WinitMapWindowConfig::new("MapLibre Quickstart".to_string()),
        WgpuSettings::default(),
        style,
        options,
    )?;
    Ok(())
}
```

### Runtime Options (`HeadedMapOptions`)

`HeadedMapOptions` configures host behaviour not carried by the MapLibre Style JSON:

- `max_frames: Option<u64>` — exits cleanly after rendering a fixed number of frames (useful for smoke tests).
- `max_pitch_degrees: f64` — clamps maximum camera tilt (`DEFAULT_MAX_PITCH` by default; raised automatically to `85°` when `style.terrain` or `Globe` projection is active unless overridden).
- `debug_tiles: bool` — installs `DebugPlugin` to draw tile borders and coordinates.
- `show_navigation_controls: bool` — toggles the interactive compass/zoom HUD in the top-right corner.
- `zoom_sensitivity: f64` — scales wheel and trackpad zoom speed (`1.0` matches MapLibre GL JS).
- `edge_scroll_enabled: bool` and `edge_scroll_speed_px_per_sec: f64` — enables cursor-at-window-edge panning.

### Custom Plugins and Live Mutations (`HeadedMapHost` & `MapHandle`)

Use `run_headed_map_with` and `HeadedMapHost` when your application registers custom [`Plugin`](../development-documents/architecture.md)s or configures the `Map` once initialized:

```rust,no_run
use std::path::PathBuf;

use maplibre::{render::settings::WgpuSettings, style::Style};
use maplibre_winit::{
    run_headed_map_with, HeadedMapHost, HeadedMapOptions, WinitMapWindowConfig,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let host = HeadedMapHost::default().with_configure(|map| {
        let _ = map.set_global_state("theme", serde_json::json!("day"));
    });

    run_headed_map_with(
        None::<PathBuf>,
        WinitMapWindowConfig::new("Configured Map".to_string()),
        WgpuSettings::default(),
        Style::default(),
        HeadedMapOptions::default(),
        host,
    )?;
    Ok(())
}
```

While a map is running (on desktop or in the browser via `web::run_map`), callers interact with it through a clonable `MapHandle<E>`:

- `map.call(|map| map.set_paint_property(layer, name, value)).await` updates paint properties, layout `visibility`, layer filters (`set_filter`), feature state (`set_feature_state`), global state (`set_global_state`), or inline GeoJSON (`set_geojson_data`) in place without rebuilding unaffected layers.
- `map.call(|map| map.fly_to(camera, curve)).await` starts smooth camera flights, and `map.until(|map| ...)` awaits frame-driven conditions such as flight arrival.
- `map.call(|map| map.query_rendered_features(geometry, &options)).await` queries visible vector and symbol features.

## Headless Rendering (`maplibre::headless`)

Enable the `headless` feature on `maplibre` to render maps to PNG buffers, host GPU textures (`HostGpu`), or synchronized multi-view slots (`MultiViewFrame`) without a display server:

```toml
[dependencies]
maplibre = { version = "0.1.0", features = ["headless"] }
```

`create_headless_renderer(width, height, cache_path)` initializes an offscreen `Kernel` and `Renderer` (backed by Metal on macOS, DX12/Vulkan on Windows, or Vulkan/Lavapipe on Linux CI), and `HeadlessMap` renders deterministic frames once all requested tiles settle.

See the [Examples](../examples/index.md) chapter for 22 runnable style, terrain, globe, clustering, heatmap, and interactive `MapHandle` examples.
