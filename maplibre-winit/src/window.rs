//! Window ownership and renderer display connection.

use maplibre::window::HeadedMapWindow;

use crate::WinitEventLoop;

pub type RawWinitWindow = winit::window::Window;

pub struct WinitMapWindow<ET: 'static> {
    pub(crate) window: RawWinitWindow,
    pub(crate) display: winit::event_loop::OwnedDisplayHandle,
    pub(crate) event_loop: Option<WinitEventLoop<ET>>,
}

impl<ET> WinitMapWindow<ET> {
    pub fn take_event_loop(&mut self) -> Option<WinitEventLoop<ET>> {
        self.event_loop.take()
    }
}

impl<ET> HeadedMapWindow for WinitMapWindow<ET> {
    type WindowHandle = RawWinitWindow;

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
