//! Window ownership and renderer display connection.
use maplibre::window::{HeadedMapWindow, MapWindow, PhysicalSize, WindowCreateError};
use std::{
    marker::PhantomData,
    sync::{Arc, Weak},
};

/// Platform window retained by each presentation surface.
pub type RawWinitWindow = winit::window::Window;

/// A drawable window whose event loop is owned by the application runner.
pub struct WinitMapWindow<ET: 'static> {
    pub(crate) window: Arc<RawWinitWindow>,
    pub(crate) display: winit::event_loop::OwnedDisplayHandle,
    pub(crate) event: PhantomData<ET>,
}

impl<ET> MapWindow for WinitMapWindow<ET> {
    fn size(&self) -> PhysicalSize {
        let size = self.window.inner_size();
        PhysicalSize::new(size.width, size.height).unwrap_or(PhysicalSize::MIN)
    }
}

impl<ET> HeadedMapWindow for WinitMapWindow<ET> {
    type WindowHandle = Arc<RawWinitWindow>;
    fn handle(&self) -> &Self::WindowHandle {
        &self.window
    }
    fn owned_display_handle(&self) -> Option<Box<dyn maplibre::window::OwnedDisplayHandle>> {
        Some(Box::new(self.display.clone()))
    }
    fn request_redraw(&self) {
        self.window.request_redraw()
    }
    fn scale_factor(&self) -> f64 {
        self.window.scale_factor()
    }
    fn id(&self) -> u64 {
        self.window.id().into()
    }
}

#[derive(Clone, Default)]
pub(crate) struct WindowBinding {
    window: Weak<RawWinitWindow>,
    display: Option<winit::event_loop::OwnedDisplayHandle>,
}
impl WindowBinding {
    pub fn new<ET>(window: &WinitMapWindow<ET>) -> Self {
        Self {
            window: Arc::downgrade(&window.window),
            display: Some(window.display.clone()),
        }
    }
    pub fn create<ET>(&self) -> Result<WinitMapWindow<ET>, WindowCreateError> {
        Ok(WinitMapWindow {
            window: self
                .window
                .upgrade()
                .ok_or(WindowCreateError::WindowNotBound)?,
            display: self
                .display
                .clone()
                .ok_or(WindowCreateError::WindowNotBound)?,
            event: PhantomData,
        })
    }
}
