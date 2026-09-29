//! Main (platform-specific) main loop which handles:
//! * Input (Mouse/Keyboard)
//! * Platform Events like suspend/resume
//! * Render a new frame

use std::marker::PhantomData;

use maplibre::window::{MapWindow, MapWindowConfig, PhysicalSize, WindowCreateError};
use winit::{dpi::Size, window::WindowAttributes};

use super::WinitMapWindow;
use crate::WinitEventLoop;

mod startup;
pub use startup::{run_headed_map, HeadedMapError};

#[derive(Clone)]
pub struct WinitMapWindowConfig<ET> {
    title: String,
    #[cfg(target_os = "android")]
    android_app: crate::android_activity::AndroidApp,

    phantom_et: PhantomData<ET>,
}

#[cfg(target_os = "android")]
impl<ET> WinitMapWindowConfig<ET> {
    pub fn new(title: String, android_app: winit::platform::android::activity::AndroidApp) -> Self {
        Self {
            title,
            android_app,
            phantom_et: Default::default(),
        }
    }
}

#[cfg(not(target_os = "android"))]
impl<ET> WinitMapWindowConfig<ET> {
    pub fn new(title: String) -> Self {
        Self {
            title,
            phantom_et: Default::default(),
        }
    }
}

impl<ET> MapWindow for WinitMapWindow<ET> {
    fn size(&self) -> PhysicalSize {
        let size = self.window.inner_size();
        #[cfg(target_os = "android")]
        // On android we can not get the dimensions of the window initially. Therefore, we use a
        // fallback until the window is ready to deliver its correct bounds.
        let window_size = PhysicalSize::new(size.width, size.height)
            .unwrap_or(PhysicalSize::new(100, 100).unwrap());

        #[cfg(not(target_os = "android"))]
        let window_size =
            PhysicalSize::new(size.width, size.height).expect("failed to get window dimensions.");
        window_size
    }
}

impl<ET: 'static + Clone> MapWindowConfig for WinitMapWindowConfig<ET> {
    type MapWindow = WinitMapWindow<ET>;

    fn create(&self) -> Result<Self::MapWindow, WindowCreateError> {
        let mut raw_event_loop_builder = winit::event_loop::EventLoop::<ET>::with_user_event();

        #[cfg(target_os = "android")]
        use winit::platform::android::EventLoopBuilderExtAndroid;
        #[cfg(target_os = "android")]
        let mut raw_event_loop_builder =
            raw_event_loop_builder.with_android_app(self.android_app.clone());

        let raw_event_loop = raw_event_loop_builder
            .build()
            .map_err(|_| WindowCreateError::EventLoop)?;

        let window = raw_event_loop
            .create_window(
                WindowAttributes::new()
                    .with_title(&self.title)
                    // TODO make window size configurable
                    .with_inner_size(Size::Logical(winit::dpi::LogicalSize::new(800.0, 800.0))),
            )
            .map_err(|_| WindowCreateError::Window)?;

        Ok(Self::MapWindow {
            window,
            display: raw_event_loop.owned_display_handle(),
            event_loop: Some(WinitEventLoop {
                event_loop: raw_event_loop,
            }),
        })
    }
}

/// Runtime options of a windowed map that the style does not carry.
#[derive(Clone, Copy, Debug)]
pub struct HeadedMapOptions {
    /// Ends the event loop after this many rendered frames, so a smoke test can drive the real
    /// windowed pipeline without input.
    pub max_frames: Option<u64>,
    /// Largest camera pitch in degrees, matching the GL JS `maxPitch` map option.
    pub max_pitch_degrees: f64,
    /// Outlines every tile of the view pattern in red, to inspect tile selection.
    pub debug_tiles: bool,
}

impl Default for HeadedMapOptions {
    fn default() -> Self {
        Self {
            max_frames: None,
            max_pitch_degrees: maplibre::render::camera::DEFAULT_MAX_PITCH.0,
            debug_tiles: false,
        }
    }
}
