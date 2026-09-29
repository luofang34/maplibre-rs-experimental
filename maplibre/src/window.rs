//! Host window handles, redraw requests and physical-to-logical viewport dimensions.

#![deny(missing_docs)]

use std::num::NonZeroU32;

use thiserror::Error;
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
    /// A clonable owner of the window and display handles retained by each presentation surface.
    type WindowHandle: wgpu::DisplayAndWindowHandle + Clone + 'static;

    /// Borrows the owner that surface creation clones to keep the platform window alive.
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
    /// The host configuration has not been bound to a live window from its active event loop.
    #[error("window configuration is not bound to a live resumed window")]
    WindowNotBound,
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
    /// Smallest drawable extent, used while a platform window has no nonzero pixel size.
    pub const MIN: Self = Self {
        width: NonZeroU32::MIN,
        height: NonZeroU32::MIN,
    };

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
    /// Clamps each dimension to one logical unit so subpixel viewports remain drawable.
    pub fn to_logical(&self, scale_factor: f64) -> LogicalSize {
        let width = self.width.get() as f64 / scale_factor;
        let height = self.height.get() as f64 / scale_factor;
        LogicalSize {
            width: NonZeroU32::new(width as u32).unwrap_or(NonZeroU32::MIN),
            height: NonZeroU32::new(height as u32).unwrap_or(NonZeroU32::MIN),
        }
    }
}

#[cfg(test)]
mod tests;
