# Running maplibre-rs demos on various platforms

During development, you will want to run the maplibre demos on your local machine to test out your changes.
There are multiple demos of maplibre-rs for different targets. Some targets have prerequisites
depending on your operating system.

* **maplibre-demo** - targets Windows, macOS and Linux, it is built directly with cargo.
* **apple** - targets iOS and macOS and relies on the xcode IDE (`apple/visionos` targets Apple Vision Pro via C ABI + Metal).
* **android** - targets 64-bit Android devices and emulators (`arm64-v8a` and `x86_64`, `minSdk 23`) in Android Studio / Gradle.
* **web** - targets browsers via WebAssembly (`wasm32-unknown-unknown`) with WebGPU and WebGL2 backends.
* **maplibre (`--features headless`)** - renders maps offscreen without a windowing system via `maplibre::headless::map::HeadlessMap` (used by server rendering, multi-view hosts, and the `render-tests` conformance runner).

All the targets below require you to install [rustup](https://rustup.rs/) to manage your Rust toolchain.

> __Note__: Make sure you have selected the right toolchain target within rustup. You can use `rustup show` to see your
> active toolchain. If you want to change the target of the build manually, use the cargo `--target` parameter.

## Maplibre-demo

### Linux/macOS

The build for desktop is very simple, you just have to run the following command from the root of the
maplibre-rs project:

```bash
cargo run -p maplibre-demo
```

To view the bundled world style on a globe, or your own style, use the `headed` options:

```bash
cargo run -p maplibre-demo -- headed --globe
cargo run -p maplibre-demo -- headed --style path/to/style.json --projection globe
cargo run -p maplibre-demo -- headed --terrain
```

Left-drag pans, the mouse wheel or `+`/`-` zooms around the pointer, right-drag or middle-drag rotates and tilts,
and `W`/`A`/`S`/`D` or the arrow keys pan along the camera's current heading (hold `Shift` for `2.5×` turbo speed).
On macOS trackpads, two-finger scrolling pans the map (like Apple Maps), pinch zooms, two-finger rotate turns the
compass bearing, and `Option` + two-finger drag tilts/orbits. A floating GPU navigation HUD in the top-right corner
provides one-click access to the 3D Compass, `2D`/`3D` (`AUTO` tilt), `+`/`-` zoom, `Home`, and `MAP`/`RTS` control
modes (where `RTS` mode enables Red Alert-style screen-edge scrolling and right-click panning). `--frames <n>` closes
the window after `n` frames.

`--terrain` opens the bundled 3D terrain style (`maplibre-demo/res/terrain.json`): OpenStreetMap raster imagery
and Mapterhorn hillshade draped over Mapterhorn `raster-dem` elevation tiles around Innsbruck at pitch 70, matching
the GL JS `add-3d-terrain` example on the globe projection. Auto-Tilt is enabled by default on terrain and globe
styles, smoothly tilting the camera as you zoom in past zoom 9 while allowing manual pitch override at any time.
As in GL JS, the globe transitions smoothly to Mercator at high zoom levels, so Earth's curvature appears when
zooming out. Scrolling zooms around the terrain under the pointer, or around the center when the pointer is in the sky;
dragging keeps the grabbed ground point under the cursor; and the camera is kept above the terrain.
Any style with a `terrain` root property and a `raster-dem` source renders the same way; the
pitch limit follows `--max-pitch` (85 degrees for `--terrain`, 60 otherwise, as in GL JS).

The web demo (`just web-lib build`, then `just web-demo start`) shows the MapLibre demotiles style
on the globe by default. `?style=<url>` loads another style and `?projection=mercator` overrides
the projection.

### Windows

Windows has two additional prerequisites to be able to run. You will need CMake, Visual Studio C++ build tools and the
sqlite3 library.

Install [CMake](https://cmake.org/download/) and add it to your path environment variables.

For the C++ build tools, download the [Visual Studio 2022 Build tools](https://visualstudio.microsoft.com/downloads/)
from the Microsoft website. After the download, while installing the Build tools, make sure that you select the
*C++ build tools*.

To install sqlite3 you need to build the sqlite3.lib manually with the following
[steps](https://gist.github.com/zeljic/d8b542788b225b1bcb5fce169ee28c55). This will generate a .lib file that
you will have to add to the SQLITE3_LIB_DIR environment variable.

Restart your shell to make sure you are using up-to-date environment variables.

Finally, the command below should execute successfully:

```bash
cargo run -p maplibre-demo
```

## Android

Start by installing the 
[Android Studio IDE](https://developer.android.com/studio?gclid=CjwKCAjwj42UBhAAEiwACIhADmF7uHXnEHGnmOgFnjp0Z6n-TnBvutC5faGA89lwouMIXiR6OXK4hBoCq78QAvD_BwE&gclsrc=aw.ds).

Make sure the NDK is installed. The Native Development Kit (NDK) is a set of tools that allows 
you to use C and C++ code with Android. You have to install manually the version that is used in 
`./android/gradle/lib/build.gradle`.

```
ANDROID STUDIO -> tools -> SDK manager -> SDK tools -> tick show package details -> ndk (side by side)
```

Open the project within `./android/gradle` and create a new virtual device with the device manager. Both `./android/gradle/lib/build.gradle` and `./android/gradle/demo/build.gradle` specify `minSdk 23` (Android 6.0+) and build exclusively for 64-bit targets (`arm64-v8a` / `aarch64-linux-android` and `x86_64` / `x86_64-linux-android`; 32-bit `i686` and `armeabi-v7a` are not supported). Install the Rust targets you need before building:

```bash
rustup target add aarch64-linux-android x86_64-linux-android
```

Finally, run the `demo` configuration in Android Studio to launch the maplibre-rs Android demo on an `x86_64` emulator or a physical `arm64-v8a` Android device.

> Note: Android is configured with `minSdk 23` and requires OpenGL ES 3.1 or Vulkan support on the target device or emulator. There are some notes
> [here](https://stackoverflow.com/questions/40797975/android-emulator-and-opengl-es3-egl-bad-config) and
> [here](https://www.reddit.com/r/Arcore/comments/8squbo/opengl_es_31_is_required_for_android_emulator_to/) that
> discuss configuring Android Studio emulators for OpenGL ES 3.1 support.
 
## Apple

Apple builds rely on the [XCode IDE](https://apps.apple.com/us/app/xcode/id497799835?ls=1&mt=12).
Start by installing XCode and open the project within `./apple/xcode`.

> Cargo is used in to build the maplibre library in the build phases of the XCode project configuration.

### iOS

You can use XCode to run on a iOS Simulator or a real device. Install a simulator in XCode.
Version 9 is the minimum version supported theoretically.

Select the scheme called *example (iOS)* and click on run. This will start the iOS application.

### macOS

As you might have seen in the maplibre-demo section, you can build Unix executables directly with Cargo.
In order to build a proper macOS application (in OSX terminology) you have to use the `./apple/xcode` project.

Open the project from the folder `./apple/xcode` with XCode. Select the scheme called *example (macOS)* and
click on run. This will start the macOS application. 

> The minimum target OSX version for the macOS build is defined inside *Build settings -> Deployment -> macOS deployment target*.
> If you are using a lower version of OSX, you will not be able to run the application on your computer.

## Web (WebGL, WebGPU)

You need to first build the library for WebGL or WebGPU. Optionally, you can also enabled multi-threading support,
which requires that the library is used in a secure environment: 
[isSecureContext](https://developer.mozilla.org/en-US/docs/Web/API/isSecureContext)
and [crossOriginIsolated](https://developer.mozilla.org/en-US/docs/Web/API/crossOriginIsolated). 
The demo runs this such an environment.

If you have a browser which already supports a recent version of the WebGPU specification then you can build the library
with WebGPU:

```bash
just web-lib build # WebGPU
```

If not, then you must enable WebGL support:


```bash
just web-lib build --webgl # WebGL
just web-lib build --webgl --multithreaded # WebGL + multithreaded
```

Instead of building it is also possible to watch for changes. The same flags as with `web-lib build` are supported:

```bash
just web-lib watch --webgl
```

After building the library you can run the demo server:

```bash
just web-demo start
```

## Headless Rendering (`maplibre --features headless`)

Offscreen rendering lives directly in the core `maplibre` crate behind `--features headless` (`maplibre::headless::create_headless_renderer` and `maplibre::headless::map::HeadlessMap`), rather than a separate crate. It renders into offscreen GPU textures or PNG/RGBA buffers on Vulkan (including Mesa Lavapipe software rasterization in CI), Metal, and DirectX 12 without creating a window:

```bash
# Run headless unit and integration tests
cargo test -p maplibre --features headless

# Run the MapLibre GL JS pixel + query conformance suite headlessly
cargo run --release -p render-tests -- --ci
```
