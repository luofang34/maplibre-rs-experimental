//! Error and change summary types for style mutations.

use thiserror::Error;

use crate::style::validation::StyleValidationError;

/// Why a change was refused.
#[derive(Debug, Error)]
pub enum StyleMutationError {
    /// A layer with this id exists already.
    #[error("style already has a layer `{layer}`")]
    DuplicateLayer {
        /// The clashing id.
        layer: String,
    },
    /// The style has no layer with this id.
    #[error("style has no layer `{layer}`")]
    UnknownLayer {
        /// The requested id.
        layer: String,
    },
    /// A source with this name exists already.
    #[error("style already has a source `{source_name}`")]
    DuplicateSource {
        /// The clashing name.
        source_name: String,
    },
    /// The style has no source with this name.
    #[error("style has no source `{source_name}`")]
    UnknownSource {
        /// The requested name.
        source_name: String,
    },
    /// A layer names a source the style does not have.
    #[error("layer `{layer}` uses source `{source_name}`, which the style does not have")]
    MissingSource {
        /// The layer.
        layer: String,
        /// The absent source.
        source_name: String,
    },
    /// A layer still draws from the source.
    #[error("source `{source_name}` is used by layer `{layer}`")]
    SourceInUse {
        /// The source.
        source_name: String,
        /// A layer using it.
        layer: String,
    },
    /// A layer that draws from a source names none.
    #[error("layer `{layer}` needs a source")]
    SourceRequired {
        /// The layer.
        layer: String,
    },
    /// A layer on a vector source names no source layer.
    #[error("layer `{layer}` needs a source-layer of vector source `{source_name}`")]
    SourceLayerRequired {
        /// The layer.
        layer: String,
        /// The vector source.
        source_name: String,
    },
    /// The layer's type cannot draw from the kind of source it names.
    #[error("layer `{layer}` of type `{kind}` cannot draw from source `{source_name}`")]
    WrongSourceKind {
        /// The layer.
        layer: String,
        /// The layer's type.
        kind: String,
        /// The source it names.
        source_name: String,
    },
    /// The lower zoom bound is above the upper one.
    #[error("layer `{layer}` has minzoom {minzoom} above maxzoom {maxzoom}")]
    InvalidZoomRange {
        /// The layer.
        layer: String,
        /// The lower bound.
        minzoom: f64,
        /// The upper bound.
        maxzoom: f64,
    },
    /// A vector, raster or DEM source given only as a TileJSON url, which is not fetched here.
    #[error("source `{source_name}` gives only a TileJSON url; list its tiles instead")]
    TileJsonNotSupported {
        /// The source.
        source_name: String,
    },
    /// The layer, or the layer with the change applied, cannot be read.
    #[error("layer `{layer}` is invalid")]
    InvalidLayer {
        /// The layer.
        layer: String,
        /// Why it cannot be read.
        #[source]
        source: serde_json::Error,
    },
    /// The layer uses something this renderer does not support.
    #[error("layer `{layer}` is not supported: {}", findings.first().map(ToString::to_string).unwrap_or_default())]
    Unsupported {
        /// The layer.
        layer: String,
        /// Everything that is not supported.
        findings: Vec<StyleValidationError>,
    },
    /// An image update names a source that is not an image source.
    #[error("source `{source_name}` is not an image source")]
    NotAnImageSource {
        /// The source.
        source_name: String,
    },
    /// A video frame update names a source that is not a video source.
    #[error("source `{source_name}` is not a video source")]
    NotAVideoSource {
        /// The source.
        source_name: String,
    },
    /// A canvas frame update names a source that is not a canvas source.
    #[error("source `{source_name}` is not a canvas source")]
    NotACanvasSource {
        /// The source.
        source_name: String,
    },
    /// A pushed RGBA frame buffer has zero dimensions or does not match `width * height * 4`.
    #[error(
        "media frame for source `{source_name}` ({width}x{height}) requires {} RGBA bytes, got {byte_len}",
        u64::from(*width) * u64::from(*height) * 4
    )]
    InvalidMediaFrame {
        /// The source.
        source_name: String,
        /// Frame width in pixels.
        width: u32,
        /// Frame height in pixels.
        height: u32,
        /// Provided RGBA slice length.
        byte_len: usize,
    },
}

/// What a change did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StyleChange {
    /// Layers that were added, removed, edited or moved.
    pub layers: Vec<String>,
    /// Whether every loaded vector tile must be fetched again to show it. New data of a GeoJSON
    /// source needs no flag: the map fetches again only the tiles the data changed in.
    pub redraw_tiles: bool,
    /// Layers that are gone, whose leftovers in loaded tiles must be dropped.
    pub removed_layers: Vec<String>,
    /// Image, video or canvas sources whose picture or corners changed, whose tiles must be made again.
    pub reloaded_image_sources: Vec<String>,
    /// Raster, image, video and canvas sources that are gone, whose tiles and textures must be dropped.
    pub removed_raster_sources: Vec<String>,
}
