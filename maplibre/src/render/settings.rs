//! Device capability requests, surface formats and GPU buffer pool capacities.

#![deny(missing_docs)]

use std::borrow::Cow;

use wgpu::PresentMode;
pub use wgpu::{Backends, Features, Limits, PowerPreference, TextureFormat};

/// Provides configuration for renderer initialization. Use [`Device::features`](wgpu::Device::features),
/// [`Device::limits`](wgpu::Device::limits), and the [`WgpuAdapterInfo`](wgpu::AdapterInfo)
/// resource to get runtime information about the actual adapter, backend, features, and limits.
#[derive(Clone)]
pub struct WgpuSettings {
    /// Label included in GPU diagnostics for the requested device.
    pub device_label: Option<Cow<'static, str>>,
    /// Backends available for adapter selection; `None` enables all compiled backends.
    /// The default reads `WGPU_BACKEND` when present.
    pub backends: Option<Backends>,
    /// Preference used when choosing a compatible adapter; not a guaranteed device class.
    pub power_preference: PowerPreference,
    /// Explicitly required features, added after automatic feature selection and exclusions.
    /// These take precedence over [`Self::disabled_features`]; unsupported requests fail initialization.
    pub features: Features,
    /// Features to remove from automatic selection, unless explicitly required by [`Self::features`].
    pub disabled_features: Option<Features>,
    /// Limits used for the device request, rather than the adapter's full capabilities.
    /// Unsupported values fail initialization unless reduced by [`Self::constrained_limits`].
    pub limits: Limits,
    /// Optional ceilings on requested capabilities: maxima decrease and minimum alignments increase.
    /// This is applied to [`Self::limits`] and does not expand the request toward adapter limits.
    pub constrained_limits: Option<Limits>,

    /// Requests a GPU trace in `wgpu_trace` under the working directory when the backend supports it.
    pub record_trace: bool,
}

impl Default for WgpuSettings {
    fn default() -> Self {
        let backends = Some(wgpu::Backends::from_env().unwrap_or(Backends::all()));

        let limits = if cfg!(feature = "web-webgl") {
            Limits {
                max_texture_dimension_2d: 4096,
                ..Limits::downlevel_webgl2_defaults()
            }
        } else if cfg!(target_os = "android") {
            Limits {
                max_storage_textures_per_shader_stage: 4,
                max_compute_workgroups_per_dimension: 0,
                max_compute_workgroup_size_z: 0,
                max_compute_workgroup_size_y: 0,
                max_compute_workgroup_size_x: 0,
                max_compute_workgroup_storage_size: 0,
                max_compute_invocations_per_workgroup: 0,
                ..Limits::downlevel_defaults()
            }
        } else {
            Limits {
                ..Limits::default()
            }
        };

        let features = if cfg!(target_arch = "wasm32") {
            Features::empty()
        } else {
            Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES
        };

        Self {
            device_label: Default::default(),
            backends,
            power_preference: PowerPreference::HighPerformance,
            features,
            disabled_features: None,
            limits,
            constrained_limits: None,
            record_trace: false,
        }
    }
}

#[derive(Clone)]
/// Whether rendering targets an offscreen texture or a host window surface.
pub enum SurfaceType {
    /// Render into a texture without presenting a window frame.
    Headless,
    /// Acquire and present frames through a host window surface.
    Headed,
}

#[derive(Copy, Clone)]
/// Configuration resource for [Multi-Sample Anti-Aliasing](https://en.wikipedia.org/wiki/Multisample_anti-aliasing).
///
pub struct Msaa {
    /// The requested number of samples for Multi-Sample Anti-Aliasing. Higher numbers result in
    /// smoother edges.
    /// Defaults to 4.
    /// WebGL uses one sample because symbol occlusion reads the depth attachment.
    pub samples: u32,
}

impl Msaa {
    /// Whether more than one sample is requested; this does not validate format support.
    pub fn is_multisampling(&self) -> bool {
        self.samples > 1
    }
}

impl Default for Msaa {
    fn default() -> Self {
        // By default we are trying to multisample
        Self { samples: 4 }
    }
}

/// Capacity of each GPU tile buffer pool in elements, not bytes.
/// Actual allocations are capped by the device's maximum buffer size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferPoolSizes {
    /// Number of geometry vertices in the vertex pool.
    pub vertices: u64,
    /// Number of draw indices in the index pool.
    pub indices: u64,
    /// Number of per-vertex feature metadata records.
    pub feature_metadata: u64,
    /// Number of layer metadata records.
    pub layer_metadata: u64,
}

impl Default for BufferPoolSizes {
    fn default() -> Self {
        Self {
            vertices: 10 * 1_000_000,
            indices: 10 * 1_000_000,
            feature_metadata: 10 * 1024 * 1000,
            layer_metadata: 10 * 1024,
        }
    }
}

#[derive(Clone, Copy)]
/// Requested render target properties and pool capacities used during renderer initialization.
pub struct RendererSettings {
    /// Requested color/depth sample count; the WebGL backend reduces this to one.
    pub msaa: Msaa,
    /// Capacity of the vector buffer pool.
    pub buffer_pools: BufferPoolSizes,
    /// Capacity of the symbol buffer pools in elements of their respective vertex/metadata types.
    pub symbol_pools: BufferPoolSizes,
    /// Requested surface format; `None` selects a supported window format or offscreen RGBA8.
    pub texture_format: Option<TextureFormat>,
    /// Depth/stencil format; initialization selects `Depth32FloatStencil8` when supported.
    pub depth_texture_format: TextureFormat,
    /// Present mode for surfaces if a surface is used.
    pub present_mode: PresentMode,
    /// Edge, in pixels, of the texture each terrain tile's map is drawn into. Each texture
    /// costs four bytes a texel and a third more for mipmaps, so the default of
    /// [`DRAPE_SIZE`](crate::terrain::resources::DRAPE_SIZE) holds about 21 MiB a tile; a
    /// phone, which shares its memory with the GPU, keeps more tiles in less.
    pub terrain_drape_size: u32,
    /// Draws every layer fragment as an equal step of grey added to what is below, so the
    /// brightness of a pixel counts how often it was drawn, as GL JS's overdraw inspector does.
    pub overdraw_inspector: bool,
}

impl RendererSettings {
    pub(super) fn with_backend_msaa(mut self, backend: wgpu::Backend) -> Self {
        // Symbol occlusion and compositor depth copies sample the depth attachment.
        // WebGL supports multisampled renderbuffers, but cannot sample their depth.
        if cfg!(target_arch = "wasm32") && backend == wgpu::Backend::Gl {
            self.msaa.samples = 1;
        }
        self
    }

    /// Selects 32-bit float depth when the device offers it.
    ///
    /// Reversed-Z only recovers precision on a float depth buffer; the 24-bit fallback keeps
    /// rendering correct but leaves depth precision at fixed-point levels.
    pub fn with_float_depth_if_supported(mut self, features: Features) -> Self {
        if features.contains(Features::DEPTH32FLOAT_STENCIL8) {
            self.depth_texture_format = TextureFormat::Depth32FloatStencil8;
        }
        self
    }
}

impl Default for RendererSettings {
    fn default() -> Self {
        Self {
            msaa: Msaa::default(),
            buffer_pools: BufferPoolSizes::default(),
            symbol_pools: BufferPoolSizes::default(),
            terrain_drape_size: crate::terrain::resources::DRAPE_SIZE,
            texture_format: None,

            depth_texture_format: TextureFormat::Depth24PlusStencil8,
            present_mode: PresentMode::AutoVsync,
            overdraw_inspector: false,
        }
    }
}
