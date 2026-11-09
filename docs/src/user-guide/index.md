# User Guide

`maplibre-rs` provides a portable, GPU-accelerated vector and 3D terrain map renderer built on `wgpu`, running natively on **macOS, Windows, Linux, iOS/iPadOS, Android, Apple Vision Pro**, and in the browser via **WebAssembly (WebGPU / WebGL2)**.

---

## Navigation HUD & Camera Controls

Windowed and web builds powered by `maplibre-winit` include a zero-dependency **GPU-rendered floating Navigation HUD** in the top-right corner of the viewport, paired with unified cross-platform input controls designed for both interactive cartography (**MAP** mode) and strategy / simulation games (**RTS** mode).

### Floating Navigation Toolbar

The top-right HUD pill (`maplibre::hud`) renders directly in the `wgpu` pass and adapts automatically to HiDPI scales and safe-area insets:

| Button | Function |
| :--- | :--- |
| **Compass** | Displays a 3D-foreshortened north-south needle tilted by the current camera pitch. **Single-click** resets bearing to North (`0°`); **double-click** resets both bearing (`0°`) and pitch (`0°`, locking `2D`). |
| **`2D` / `3D` + `AUTO`** | Toggles between flat top-down (`0°` pitch) and tilted 3D perspective. On 3D Terrain and Globe styles, an illuminated **`AUTO`** sub-badge indicates that zoom-driven **Auto-Tilt** is active. Clicking `2D`/`3D` when `AUTO` is off re-engages Auto-Tilt on terrain/globe styles. |
| **`+` / `-`** | Smoothly zooms in or out by 1 zoom level around the viewport center. |
| **Home (`⌂`)** | Smoothly flies back to the style's initial center, zoom, pitch, and bearing, and re-enables Auto-Tilt if supported by the style. |
| **`MAP` / `RTS`** | Switches the mouse and edge-scrolling control profile between standard map navigation (**`MAP`**) and Real-Time Strategy camera ergonomics (**`RTS`**). |

### Automatic Zoom-Driven Tilt (`Auto-Tilt`)

On styles with 3D terrain (`"terrain"`) or `"globe"` projection, **Auto-Tilt** is enabled by default (`AUTO` badge lit in green):

- **Zoom `≤ 9.0`**: Camera remains flat (`0°` pitch) for an unobstructed global or regional overview.
- **Zoom `9.0 → 14.5`**: Camera smoothly interpolates pitch along a cubic Hermite curve from `0°` up to `58°` as you zoom in toward mountains, valleys, and city streets.
- **Zoom `14.5 → 16.5`**: Continues tilting smoothly up to `65°`.
- **Manual Override**: Manually tilting the camera (via right/middle mouse drag, `Option` + trackpad drag, two-finger vertical touch drag, or `R`/`F` keys) automatically disengages `AUTO` and preserves your custom pitch until you click the `2D`/`3D` HUD button or **Home (`⌂`)**.

---

## Cross-Platform Input Controls

### Desktop Mouse & Keyboard

| Action | `MAP` Profile (Default) | `RTS` Profile (Strategy / Simulation) |
| :--- | :--- | :--- |
| **Pan Map** | **Left-click drag** (ground-point anchored) | **Right-click drag** or **Left-click drag** |
| **Orbit (Bearing & Pitch)** | **Right-click drag** or **Middle-click drag** | **Middle-click drag** or **`Alt`/`Option` + Left-drag** |
| **Zoom** | **Mouse wheel** (around cursor ground point) | **Mouse wheel** (around cursor ground point) |
| **Screen-Edge Scroll** | Optional (`edge_scroll_enabled`) | **Active by default** when cursor nears window edges (`36 px` zone) |
| **Keyboard Pan** | **`W`/`A`/`S`/`D`** or **Arrow keys** | **`W`/`A`/`S`/`D`** or **Arrow keys** |
| **Keyboard Orbit / Tilt** | **`Q`/`E`** (rotate bearing), **`R`/`F`** (tilt pitch) | **`Q`/`E`** (rotate bearing), **`R`/`F`** (tilt pitch) |
| **Turbo Speed (`2.5×`)** | Hold **`Shift`** during keyboard pan or edge-scroll | Hold **`Shift`** during keyboard pan or edge-scroll |
| **Reset Orientation** | **`N`** (reset North), **`0`** or **`H`** (reset Home) | **`N`** (reset North), **`0`** or **`H`** (reset Home) |

> **Camera-Aligned Edge & Keyboard Scrolling**: Both keyboard panning and screen-edge scrolling follow the camera's current **bearing** and **pitch**—pushing the mouse to the top edge always advances toward the horizon the camera is facing, and pushing to the bottom edge retreats toward the camera nadir.

### macOS Trackpad (`winit` Gesture Recognition)

`maplibre-winit` automatically distinguishes multi-touch trackpad scroll streams from discrete mouse scroll wheels:

- **Two-Finger Scroll (Horizontal & Vertical)**: Smoothly pans the map in any direction (matching **Apple Maps** on macOS).
- **Two-Finger Pinch (`TouchpadMagnify`)**: Smoothly zooms in/out anchored to the ground point under the pointer (or **`Cmd` / `Ctrl` + two-finger vertical scroll**).
- **Two-Finger Rotate (`TouchpadRotate`)**: Rotates the camera bearing around the pointer ground point.
- **`Option` (`Alt`) + Two-Finger Scroll**: Tilts camera pitch (vertical) and orbits bearing (horizontal), pausing Auto-Tilt for manual inspection.

### Touch Devices (iPhone, iPad, Android, Touch Web)

- **1-Finger Drag**: Ground-anchored pan.
- **2-Finger Pinch & Rotate**: Simultaneous zoom and bearing rotation around the pinch midpoint.
- **2-Finger Parallel Vertical Drag**: Tilts the camera pitch (`0° ↔ max_pitch`), automatically disengaging Auto-Tilt.
- **Double-Tap (1 Finger)**: Smoothly zooms in (`+1` level) at the tapped location.
- **Two-Finger Tap**: Smoothly zooms out (`-1` level).
- **Double-Tap & Drag (1 Finger)**: One-handed continuous vertical zoom.

---

## Configuring Controls in Downstream Applications

Downstream applications and games using [`HeadedMapOptions`](../api/maplibre_winit/struct.HeadedMapOptions.html) can customize the HUD visibility, Auto-Tilt behavior, zoom speed, and edge-scrolling parameters:

```rust,no_run
use std::path::PathBuf;
use maplibre::{render::settings::WgpuSettings, style::Style};
use maplibre_winit::{run_headed_map, HeadedMapOptions, WinitMapWindowConfig};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let style = Style::default();

    run_headed_map(
        Some(PathBuf::from("./maplibre-cache")),
        WinitMapWindowConfig::new("Custom RTS Map".to_string()),
        WgpuSettings::default(),
        style,
        HeadedMapOptions {
            max_pitch_degrees: 85.0,
            // Show or hide the top-right GPU navigation HUD
            show_navigation_controls: true,
            // Force Auto-Tilt on (`Some(true)`), off (`Some(false)`), or automatic per style (`None`)
            auto_tilt_override: Some(true),
            // Scale scroll-wheel and pinch zoom speed (1.0 = default)
            zoom_sensitivity: 1.0,
            // Enable RTS screen-edge scrolling and customize its base speed (physical px/sec)
            edge_scroll_enabled: true,
            edge_scroll_speed_px_per_sec: 1100.0,
            ..Default::default()
        },
    )?;
    Ok(())
}
```

---

## Predictive Tile Streaming for Pitched 3D Views

At steep camera pitches (`60°–85°`), tiles entering the bottom edge of the screen (near the camera nadir) are geometrically much closer to the eye than tiles near the top horizon—meaning each screen pixel at the bottom edge traverses world space several times faster during backward scrolling.

To prevent blank or unbaked tiles when edge-scrolling backward (`S` / Down arrow / bottom screen edge), `maplibre-rs` employs a four-stage predictive streaming pipeline:

1. **Pitched Nadir Apron (`ViewState::pitched_nadir_apron`)**: Whenever camera pitch exceeds `20°`, the loose tile covering frustum automatically unions a nadir-shifted camera apron (`0.68 × d_nadir` behind the bottom viewport edge), pre-queuing DEM and vector/raster tiles before you even move.
2. **Velocity-Predictive `PrefetchView`**: `maplibre-winit` tracks an exponentially smoothed pan velocity vector and projects a predictive camera `0.80 s` ahead of motion (plus an additional nadir lead when retreating), unioning its tile coverage into the loose request pass.
3. **GPU Drape Pre-Baking (`DrapePrefetchRequests`)**: Off-screen tiles in the loose covering ring are pre-baked into `2048×2048` terrain drape textures on the GPU (`64 MiB` cache budget) before they cross into the visible viewport.
4. **Coarse `z - 3` Raster Overview Fallback**: Terrain styles automatically prefetch and retain a low-cost `zoom - 3` coarse raster ancestor pyramid (`1 tile` per `64` fine tiles), guaranteeing instantaneous terrain-draped imagery even under rapid camera slews.
