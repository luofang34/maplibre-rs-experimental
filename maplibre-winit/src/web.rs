//! Canvas windows are created only on the active browser event loop.
use std::{marker::PhantomData, sync::Arc};

use maplibre::window::{MapWindowConfig, WindowCreateError};
use winit::{platform::web::WindowAttributesExtWebSys, window::WindowAttributes};

use crate::{WinitHostError, WinitMapWindow};

/// Canvas selection and binding for a browser map.
#[derive(Clone)]
pub struct WinitMapWindowConfig<ET> {
    canvas_id: String,
    binding: crate::window::WindowBinding,
    phantom_et: PhantomData<ET>,
}
impl<ET: 'static + Clone> WinitMapWindowConfig<ET> {
    /// Selects an existing DOM canvas; the active event loop creates its window.
    pub fn new(canvas_id: String) -> Self {
        Self {
            canvas_id,
            binding: Default::default(),
            phantom_et: PhantomData,
        }
    }
    pub(crate) fn event_loop(&self) -> Result<crate::RawWinitEventLoop<ET>, WinitHostError> {
        winit::event_loop::EventLoop::<ET>::with_user_event()
            .build()
            .map_err(WinitHostError::EventLoop)
    }
    /// Creates the canvas window during the application's `resumed` callback.
    pub fn create_window(
        &self,
        active: &winit::event_loop::ActiveEventLoop,
    ) -> Result<WinitMapWindow<ET>, WinitHostError> {
        let canvas = get_canvas(&self.canvas_id)?;
        let window = active
            .create_window(WindowAttributes::default().with_canvas(Some(canvas)))
            .map_err(WinitHostError::Window)?;
        Ok(WinitMapWindow {
            window: Arc::new(window),
            display: active.owned_display_handle(),
            event: PhantomData,
        })
    }
    /// Binds map construction to a live canvas window without extending its lifetime.
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

/// Resolves the configured canvas, preserving its ID in DOM selection errors.
pub fn get_canvas(element_id: &str) -> Result<web_sys::HtmlCanvasElement, WinitHostError> {
    use wasm_bindgen::JsCast;
    let document = web_sys::window()
        .and_then(|window| window.document())
        .ok_or(WinitHostError::DocumentUnavailable)?;
    document
        .get_element_by_id(element_id)
        .ok_or_else(|| WinitHostError::CanvasMissing {
            id: element_id.into(),
        })?
        .dyn_into::<web_sys::HtmlCanvasElement>()
        .map_err(|_| WinitHostError::InvalidCanvas {
            id: element_id.into(),
        })
}
