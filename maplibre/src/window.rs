//! Host window handles, redraw requests and physical-to-logical viewport dimensions.

#![deny(missing_docs)]

use std::num::NonZeroU32;

use thiserror::Error;
use wgpu::rwh::{HasDisplayHandle, HasWindowHandle};
/// An owned platform connection suitable for GPU instance creation.
pub use wgpu::wgt::WgpuHasDisplayHandle as OwnedDisplayHandle;

/// Window of a certain [`PhysicalSize`]. This can either be a proper window or a headless one.
pub trait MapWindow {
    /// Drawable dimensions in physical pixels; neither dimension may be zero.
    fn size(&self) -> PhysicalSize;
}

/// Window which references a physical `RawWindow`. This is only implemented by headed windows and
/// not by headless windows.
pub trait HeadedMapWindow: MapWindow {
    /// Platform handles borrowed by surface creation while the window remains alive.
    type WindowHandle: HasWindowHandle + HasDisplayHandle + Sync;

    /// Borrows the window and display handles without transferring ownership to the renderer.
    fn handle(&self) -> &Self::WindowHandle;

    /// Owns the display connection used to create this window, where the backend needs one.
    fn owned_display_handle(&self) -> Option<Box<dyn OwnedDisplayHandle>> {
        None
    }

    /// Asks the host event loop to schedule a redraw; it need not draw synchronously.
    fn request_redraw(&self);

    /// Number of physical pixels per logical window unit.
    fn scale_factor(&self) -> f64;

    /// Host-defined identifier used to associate events with this window.
    fn id(&self) -> u64;
}

#[derive(Error, Debug)]
/// Failure to create the event loop or window required by a host configuration.
pub enum WindowCreateError {
    /// The host could not create its event loop.
    #[error("unable to create event loop")]
    EventLoop,
    /// The host could not create its drawable window.
    #[error("unable to create window")]
    Window,
}

/// A configuration for a window which determines the corresponding implementation of a
/// [`MapWindow`] and is able to create it.
pub trait MapWindowConfig: 'static + Clone {
    /// Window implementation produced by this configuration.
    type MapWindow: MapWindow;

    /// Creates the host window, reporting event-loop or window creation failure.
    fn create(&self) -> Result<Self::MapWindow, WindowCreateError>;
}

/// Nonzero drawable dimensions in physical pixels.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub struct PhysicalSize {
    width: NonZeroU32,
    height: NonZeroU32,
}

impl PhysicalSize {
    /// Returns `None` if either physical-pixel dimension is zero.
    pub fn new(width: u32, height: u32) -> Option<Self> {
        Some(Self {
            width: NonZeroU32::new(width)?,
            height: NonZeroU32::new(height)?,
        })
    }

    /// Horizontal extent in physical pixels.
    pub fn width(&self) -> u32 {
        self.width.get()
    }

    /// Horizontal extent with its nonzero invariant retained in the type.
    pub fn width_non_zero(&self) -> NonZeroU32 {
        self.width
    }

    /// Vertical extent in physical pixels.
    pub fn height(&self) -> u32 {
        self.height.get()
    }

    /// Vertical extent with its nonzero invariant retained in the type.
    pub fn height_non_zero(&self) -> NonZeroU32 {
        self.height
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
/// Nonzero integer viewport dimensions in logical window units, before device scaling.
pub struct LogicalSize {
    width: NonZeroU32,
    height: NonZeroU32,
}

impl LogicalSize {
    /// Returns `None` if either logical dimension is zero.
    pub fn new(width: u32, height: u32) -> Option<Self> {
        Some(Self {
            width: NonZeroU32::new(width)?,
            height: NonZeroU32::new(height)?,
        })
    }

    /// Horizontal extent in logical window units.
    pub fn width(&self) -> u32 {
        self.width.get()
    }

    /// Logical horizontal extent with its nonzero invariant retained in the type.
    pub fn width_non_zero(&self) -> NonZeroU32 {
        self.width
    }

    /// Vertical extent in logical window units.
    pub fn height(&self) -> u32 {
        self.height.get()
    }

    /// Logical vertical extent with its nonzero invariant retained in the type.
    pub fn height_non_zero(&self) -> NonZeroU32 {
        self.height
    }
}

impl PhysicalSize {
    /// Divides by the device scale and truncates fractional logical units.
    ///
    /// # Panics
    /// Panics if either converted dimension becomes zero. The caller must supply a positive,
    /// finite scale that leaves at least one logical unit in each dimension.
    pub fn to_logical(&self, scale_factor: f64) -> LogicalSize {
        let width = self.width.get() as f64 / scale_factor;
        let height = self.height.get() as f64 / scale_factor;
        LogicalSize {
            width: NonZeroU32::new(width as u32).expect("impossible to reach"),
            height: NonZeroU32::new(height as u32).expect("impossible to reach"),
        }
    }
}
