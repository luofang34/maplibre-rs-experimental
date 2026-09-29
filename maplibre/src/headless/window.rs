//! Fixed physical dimensions for a map rendered into an offscreen texture.

#![deny(missing_docs)]

use crate::window::{MapWindow, MapWindowConfig, PhysicalSize, WindowCreateError};

#[derive(Clone)]
/// Factory for an offscreen window with fixed, nonzero physical dimensions.
pub struct HeadlessMapWindowConfig {
    size: PhysicalSize,
}

impl HeadlessMapWindowConfig {
    /// Stores the target texture dimensions in physical pixels.
    pub fn new(size: PhysicalSize) -> Self {
        Self { size }
    }
}

impl MapWindowConfig for HeadlessMapWindowConfig {
    type MapWindow = HeadlessMapWindow;

    fn create(&self) -> Result<Self::MapWindow, WindowCreateError> {
        Ok(Self::MapWindow { size: self.size })
    }
}

/// Offscreen window dimensions without native window or display handles.
pub struct HeadlessMapWindow {
    size: PhysicalSize,
}

impl MapWindow for HeadlessMapWindow {
    fn size(&self) -> PhysicalSize {
        self.size
    }
}
