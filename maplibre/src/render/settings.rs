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

impl BufferPoolSizes {
    /// Zero-capacity pool configuration that disables geometry uploads for a pool family
    /// without creating invalid zero-sized GPU buffers.
    pub const EMPTY: Self = Self {
        vertices: 0,
        indices: 0,
        feature_metadata: 0,
        layer_metadata: 0,
    };

    /// Returns `true` when every pool sub-allocation capacity is zero.
    pub const fn is_empty(&self) -> bool {
        self.vertices == 0
            && self.indices == 0
            && self.feature_metadata == 0
            && self.layer_metadata == 0
    }

    /// Right-sized capacity ceiling for the SDF symbol/icon buffer pool.
    pub const fn default_symbols() -> Self {
        #[cfg(target_arch = "wasm32")]
        {
            Self {
                vertices: 229_376,
                indices: 344_064,
                feature_metadata: 229_376,
                layer_metadata: 4_096,
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            Self {
                vertices: 1_048_576,
                indices: 1_572_864,
                feature_metadata: 1_048_576,
                layer_metadata: 16_384,
            }
        }
    }
}

impl Default for BufferPoolSizes {
    fn default() -> Self {
        #[cfg(target_arch = "wasm32")]
        {
            Self {
                vertices: 1_250_000,
                indices: 3_750_000,
                feature_metadata: 131_072,
                layer_metadata: 8_192,
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            Self {
                vertices: 4_000_000,
                indices: 12_000_000,
                feature_metadata: 512_000,
                layer_metadata: 32_768,
            }
        }
    }
}

/// Styling parameters for [`TerrainDrapeMode::ProceduralRelief`], shading pure 3D DEM relief
/// directly in `MAIN_PASS` without allocating `DrapeScratch` or `DrapeCache`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProceduralReliefStyle {
    /// Optional host reference altitude in metres MSL (e.g. ownship altitude for TAWS
    /// relative-elevation amber/red terrain awareness bands). `None` disables TAWS bands.
    pub reference_altitude_meters: Option<f32>,
    /// Enables hypsometric elevation color ramp based on stitched DEM elevation (metres MSL).
    pub hypsometric: bool,
    /// Analytical DEM surface-normal hillshade strength (`0.0..=1.0`, default `0.65`).
    pub slope_shading: f32,
    /// Ground metric gridline spacing in metres (`<= 0.0` disables gridlines; default `1000.0`).
    pub grid_spacing_meters: f32,
    /// Opacity of procedural metric gridlines (`0.0..=1.0`, default `0.18`).
    pub grid_opacity: f32,
}

impl Default for ProceduralReliefStyle {
    fn default() -> Self {
        Self {
            reference_altitude_meters: None,
            hypsometric: true,
            slope_shading: 0.65,
            grid_spacing_meters: 1000.0,
            grid_opacity: 0.18,
        }
    }
}

/// Controls whether 3D terrain bakes per-tile 2D style drapes (`DRAPE_PASS`) or shades pure
/// procedural DEM relief directly in `MAIN_PASS`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TerrainDrapeMode {
    /// Bakes drapeable style layers into mipmapped per-tile textures of `edge_px × edge_px`.
    Draped {
        /// Edge length in pixels of each drape texture (clamped to `>= MIN_DRAPE_SIZE`).
        edge_px: u32,
    },
    /// Allocates zero `DrapeScratch` and zero `DrapeCache` textures, shading stitched DEM
    /// elevation, analytical surface normal, optional TAWS relative-altitude bands, and
    /// procedural metric gridlines directly in `MAIN_PASS`.
    ProceduralRelief(ProceduralReliefStyle),
}

impl TerrainDrapeMode {
    /// Default [`TerrainDrapeMode::ProceduralRelief`] with hypsometric bands, analytical slope
    /// shading, and `1 km` gridlines.
    pub const fn procedural_relief() -> Self {
        Self::ProceduralRelief(ProceduralReliefStyle {
            reference_altitude_meters: None,
            hypsometric: true,
            slope_shading: 0.65,
            grid_spacing_meters: 1000.0,
            grid_opacity: 0.18,
        })
    }

    /// Returns `true` when drape baking is disabled in favor of direct procedural DEM relief.
    pub const fn is_procedural_relief(&self) -> bool {
        matches!(self, Self::ProceduralRelief(_))
    }
}

impl Default for TerrainDrapeMode {
    fn default() -> Self {
        Self::Draped {
            edge_px: crate::terrain::resources::DRAPE_SIZE,
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
    /// [`DRAPE_SIZE`](crate::terrain::resources::DRAPE_SIZE) holds about 5.6 MiB a tile; a
    /// phone, which shares its memory with the GPU, keeps more tiles in less.
    pub terrain_drape_size: u32,
    /// Whether terrain bakes 2D style drapes (`DRAPE_PASS`) or shades procedural DEM relief
    /// directly in `MAIN_PASS` (`TerrainDrapeMode::ProceduralRelief`).
    pub terrain_drape_mode: TerrainDrapeMode,
    /// Draws every layer fragment as an equal step of grey added to what is below, so the
    /// brightness of a pixel counts how often it was drawn, as GL JS's overdraw inspector does.
    pub overdraw_inspector: bool,
    /// Allocates and populates a persistent single-sample reversed-Z `Depth32Float` attachment
    /// via `DEPTH_COPY` on offscreen targets so hosts can sample or copy depth without `XrFrame`.
    pub depth_readout: bool,
    /// Omits the 1-tile surround prefetch ring when requesting tiles for stationary one-shot
    /// offscreen snapshots.
    pub tight_tile_requests: bool,
}

/// Resource mirroring [`RendererSettings::tight_tile_requests`] for world-only prefetch helpers.
#[derive(Clone, Copy, Debug, Default)]
pub struct TightTileRequests(pub bool);

impl RendererSettings {
    /// Resolves the active [`TerrainDrapeMode`], honoring legacy `terrain_drape_size` overrides
    /// when `terrain_drape_mode` was left at its default.
    pub fn effective_terrain_drape_mode(&self) -> TerrainDrapeMode {
        match self.terrain_drape_mode {
            TerrainDrapeMode::ProceduralRelief(style) => TerrainDrapeMode::ProceduralRelief(style),
            TerrainDrapeMode::Draped { edge_px }
                if edge_px != crate::terrain::resources::DRAPE_SIZE =>
            {
                TerrainDrapeMode::Draped { edge_px }
            }
            TerrainDrapeMode::Draped { .. } => TerrainDrapeMode::Draped {
                edge_px: self.terrain_drape_size,
            },
        }
    }

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
            symbol_pools: BufferPoolSizes::default_symbols(),
            terrain_drape_size: crate::terrain::resources::DRAPE_SIZE,
            terrain_drape_mode: TerrainDrapeMode::default(),
            texture_format: None,

            depth_texture_format: TextureFormat::Depth24PlusStencil8,
            present_mode: PresentMode::AutoVsync,
            overdraw_inspector: false,
            depth_readout: false,
            tight_tile_requests: false,
        }
    }
}
