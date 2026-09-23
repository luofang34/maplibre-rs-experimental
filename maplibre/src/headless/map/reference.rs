//! Colour and metric depth rendered from a calibrated pinhole camera.
//!
//! A reference render is the map as a real camera with the given intrinsics
//! and pose would see it. Visual positioning compares a camera frame with such
//! a render. [`ReferenceTarget`] owns the depth attachment, draws the view, and
//! reads it back. Depth is the distance along the optical axis in metres.

mod readback;

use std::time::Duration;

use cgmath::{Matrix4, SquareMatrix};
pub use readback::{read as read_texture, read_blocking as read_texture_blocking, ReadbackError};
use thiserror::Error;

use crate::{
    headless::map::{xr::XrFrameError, HeadlessMap},
    render::{
        camera::EyeFrustum,
        view_state::ExternalAnchor,
        xr::{EyeTarget, ScenePlacement, XrEye, XrFrame},
    },
};

/// Near clip distance of every reference render, in metres.
pub const REFERENCE_NEAR_M: f64 = 10.0;
/// Far clip distance of every reference render, in metres.
pub const REFERENCE_FAR_M: f64 = 1_000_000.0;
/// Frames drawn per reference so that supplied tiles and terrain are resident.
pub const REFERENCE_SETTLE_FRAMES: u32 = 4;
/// Time between settle frames. Frames continue from the map's last frame time,
/// so reference and display frames share one monotonic clock.
const SETTLE_FRAME_INTERVAL: Duration = Duration::from_millis(16);

/// Intrinsics of an undistorted pinhole camera, in pixels.
///
/// Pixel centres have integer coordinates, and rows increase down the image.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PinholeIntrinsics {
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Horizontal focal length in pixels.
    pub fx: f64,
    /// Vertical focal length in pixels.
    pub fy: f64,
    /// Principal point column.
    pub cx: f64,
    /// Principal point row.
    pub cy: f64,
}

impl PinholeIntrinsics {
    /// The asymmetric frustum whose pixel grid matches these intrinsics.
    pub fn frustum(&self) -> EyeFrustum {
        EyeFrustum {
            left: (self.cx + 0.5) / self.fx,
            right: (f64::from(self.width) - self.cx - 0.5) / self.fx,
            top: (self.cy + 0.5) / self.fy,
            bottom: (f64::from(self.height) - self.cy - 0.5) / self.fy,
            near: REFERENCE_NEAR_M,
            far: REFERENCE_FAR_M,
        }
    }

    fn validate(&self) -> Result<(), ReferenceError> {
        let finite = [self.fx, self.fy, self.cx, self.cy]
            .iter()
            .all(|v| v.is_finite());
        if self.width == 0 || self.height == 0 || !finite || self.fx <= 0.0 || self.fy <= 0.0 {
            return Err(ReferenceError::InvalidIntrinsics);
        }
        Ok(())
    }
}

/// Why a reference render failed.
#[derive(Debug, Error)]
pub enum ReferenceError {
    /// The intrinsics do not define a camera.
    #[error("reference intrinsics must have a size and positive finite focal lengths")]
    InvalidIntrinsics,
    /// The map renders at a different size than the intrinsics.
    #[error("map renders {map:?} pixels but the reference camera has {camera:?}")]
    SizeMismatch {
        /// Size of the map's colour texture.
        map: (u32, u32),
        /// Size of the reference camera.
        camera: (u32, u32),
    },
    /// The map has no colour texture to read.
    #[error("the map has no colour texture")]
    MissingColour,
    /// Drawing the view failed.
    #[error("reference view could not be drawn")]
    Draw(#[from] XrFrameError),
    /// Reading the colour or depth texture failed.
    #[error("reference view could not be read back")]
    Readback(#[from] ReadbackError),
}

/// Colour and optical-axis depth of one reference view, row-major.
#[derive(Clone, Debug, PartialEq)]
pub struct ReferenceRender {
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// RGBA colour. Alpha below 255 marks missing imagery.
    pub rgba: Vec<u8>,
    /// Depth along the optical axis in metres. Zero marks no rendered surface
    /// or missing imagery.
    pub depth_m: Vec<f32>,
}

/// The depth attachment and draw state of reference renders for one camera.
pub struct ReferenceTarget {
    intrinsics: PinholeIntrinsics,
    depth: wgpu::Texture,
}

impl ReferenceTarget {
    /// Create the depth attachment for a camera. The map must render at its size.
    ///
    /// The render size and field of view select the tile zoom that the map
    /// draws. A supplied tile at another zoom is not used, so a small render of
    /// a package with only fine tiles can show no imagery and no depth.
    pub fn new(map: &HeadlessMap, intrinsics: PinholeIntrinsics) -> Result<Self, ReferenceError> {
        intrinsics.validate()?;
        let depth = map.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("reference depth"),
            size: wgpu::Extent3d {
                width: intrinsics.width,
                height: intrinsics.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        Ok(Self { intrinsics, depth })
    }

    /// Intrinsics of this target.
    pub fn intrinsics(&self) -> PinholeIntrinsics {
        self.intrinsics
    }

    /// Draw the view from `world_from_eye` in the frame anchored at `anchor`.
    pub fn draw(
        &self,
        map: &mut HeadlessMap,
        anchor: ExternalAnchor,
        world_from_eye: Matrix4<f64>,
    ) -> Result<(), ReferenceError> {
        self.check_size(map)?;
        for _ in 0..REFERENCE_SETTLE_FRAMES {
            let timestamp = map.frame_input_mut().timestamp + SETTLE_FRAME_INTERVAL;
            map.run_xr_frame(XrFrame {
                timestamp,
                opaque_environment: true,
                placement: ScenePlacement {
                    anchor,
                    world_from_scene: Matrix4::identity(),
                },
                eyes: vec![XrEye {
                    world_from_eye,
                    frustum: self.intrinsics.frustum(),
                    target: EyeTarget {
                        color: None,
                        depth: Some(self.depth.create_view(&Default::default())),
                    },
                }],
                request_overscan: 1.0,
                prefetch: None,
            })?;
        }
        Ok(())
    }

    /// Read the last drawn view. The browser drives the mapping, so this suits WebGPU.
    ///
    /// On native targets use [`Self::read_blocking`]; this future waits for a
    /// device poll that it does not issue.
    pub async fn read(&self, map: &HeadlessMap) -> Result<ReferenceRender, ReferenceError> {
        let colour = map.head_texture().ok_or(ReferenceError::MissingColour)?;
        let rgba = readback::read(map, colour, wgpu::TextureAspect::All).await?;
        let depth = readback::read(map, &self.depth, wgpu::TextureAspect::DepthOnly).await?;
        Ok(self.assemble(rgba, &depth))
    }

    /// Read the last drawn view and wait for the device.
    pub fn read_blocking(&self, map: &HeadlessMap) -> Result<ReferenceRender, ReferenceError> {
        let colour = map.head_texture().ok_or(ReferenceError::MissingColour)?;
        let rgba = readback::read_blocking(map, colour, wgpu::TextureAspect::All)?;
        let depth = readback::read_blocking(map, &self.depth, wgpu::TextureAspect::DepthOnly)?;
        Ok(self.assemble(rgba, &depth))
    }

    /// Draw and read one view on a native target.
    pub fn render_blocking(
        &self,
        map: &mut HeadlessMap,
        anchor: ExternalAnchor,
        world_from_eye: Matrix4<f64>,
    ) -> Result<ReferenceRender, ReferenceError> {
        self.draw(map, anchor, world_from_eye)?;
        self.read_blocking(map)
    }

    fn check_size(&self, map: &HeadlessMap) -> Result<(), ReferenceError> {
        let camera = (self.intrinsics.width, self.intrinsics.height);
        let texture = map.head_texture().ok_or(ReferenceError::MissingColour)?;
        let size = (texture.width(), texture.height());
        if size != camera {
            return Err(ReferenceError::SizeMismatch { map: size, camera });
        }
        Ok(())
    }

    fn assemble(&self, rgba: Vec<u8>, depth: &[u8]) -> ReferenceRender {
        let depth_m = depth
            .chunks_exact(4)
            .zip(rgba.chunks_exact(4))
            .map(|(raw, colour)| {
                let raw = f32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
                if colour[3] == 255 {
                    optical_depth_m(raw)
                } else {
                    0.0
                }
            })
            .collect();
        ReferenceRender {
            width: self.intrinsics.width,
            height: self.intrinsics.height,
            rgba,
            depth_m,
        }
    }
}

/// Optical-axis depth in metres from a reversed-Z depth sample of a reference render.
///
/// Returns zero for the far plane, for samples outside the depth range, and
/// for non-finite samples.
pub fn optical_depth_m(reversed_z: f32) -> f32 {
    let d = f64::from(reversed_z);
    if !(d > 0.0 && d <= 1.0) {
        return 0.0;
    }
    let (near, far) = (REFERENCE_NEAR_M, REFERENCE_FAR_M);
    (near * far / (near + d * (far - near))) as f32
}

#[cfg(test)]
mod tests;
