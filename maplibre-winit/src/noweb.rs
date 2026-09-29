//! Main (platform-specific) main loop which handles:
//! * Input (Mouse/Keyboard)
//! * Platform Events like suspend/resume
//! * Render a new frame

use std::{marker::PhantomData, sync::Arc};

use maplibre::window::{MapWindowConfig, WindowCreateError};
use winit::{dpi::Size, window::WindowAttributes};

use super::WinitMapWindow;

mod startup;
pub use startup::{run_headed_map, HeadedMapError};

/// Native window title and binding used to construct maps during a resumed callback.
#[derive(Clone)]
pub struct WinitMapWindowConfig<ET> {
    title: String,
    binding: crate::window::WindowBinding,
    #[cfg(target_os = "android")]
    android_app: crate::android_activity::AndroidApp,

    phantom_et: PhantomData<ET>,
}

#[cfg(target_os = "android")]
impl<ET> WinitMapWindowConfig<ET> {
    /// Selects the native activity and window title without creating platform resources.
    pub fn new(title: String, android_app: winit::platform::android::activity::AndroidApp) -> Self {
        Self {
            title,
            binding: Default::default(),
            android_app,
            phantom_et: Default::default(),
        }
    }
}

#[cfg(not(target_os = "android"))]
impl<ET> WinitMapWindowConfig<ET> {
    /// Selects a window title; creation is deferred until the host event loop resumes.
    pub fn new(title: String) -> Self {
        Self {
            title,
            binding: Default::default(),
            phantom_et: Default::default(),
        }
    }
}

impl<ET: 'static + Clone> WinitMapWindowConfig<ET> {
    pub(crate) fn event_loop(&self) -> Result<crate::RawWinitEventLoop<ET>, crate::WinitHostError> {
        let mut builder = winit::event_loop::EventLoop::<ET>::with_user_event();
        #[cfg(target_os = "android")]
        {
            use winit::platform::android::EventLoopBuilderExtAndroid;
            builder.with_android_app(self.android_app.clone());
        }
        builder.build().map_err(crate::WinitHostError::EventLoop)
    }
    /// Creates the window during the application's `resumed` callback.
    pub fn create_window(
        &self,
        active: &winit::event_loop::ActiveEventLoop,
    ) -> Result<WinitMapWindow<ET>, crate::WinitHostError> {
        let window = active
            .create_window(
                WindowAttributes::default()
                    .with_title(&self.title)
                    .with_inner_size(Size::Logical(winit::dpi::LogicalSize::new(800.0, 800.0))),
            )
            .map_err(crate::WinitHostError::Window)?;
        Ok(WinitMapWindow {
            window: Arc::new(window),
            display: active.owned_display_handle(),
            event: PhantomData,
        })
    }
    /// Binds map construction to a live window without extending that window's lifetime.
    pub fn with_window(mut self, window: &WinitMapWindow<ET>) -> Self {
        self.binding = crate::window::WindowBinding::new(window);
        self
    }
}

impl<ET: 'static + Clone> MapWindowConfig for WinitMapWindowConfig<ET> {
    type MapWindow = WinitMapWindow<ET>;
    fn create(&self) -> Result<Self::MapWindow, WindowCreateError> {
        self.binding.create()
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
