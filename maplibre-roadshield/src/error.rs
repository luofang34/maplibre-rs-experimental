//! Why a pack does not load or a shield is not made.

use std::path::PathBuf;

/// A resource pack directory cannot be read or is not a valid pack.
#[derive(Debug, thiserror::Error)]
pub enum PackLoadError {
    /// A file or directory of the pack cannot be read.
    #[error("cannot read road shield pack file {path}")]
    Io {
        /// The file or directory.
        path: PathBuf,
        /// The I/O failure.
        #[source]
        source: std::io::Error,
    },
    /// The files do not form a pack whose hashes and rules check out.
    #[error("invalid road shield pack in {dir}")]
    Pack {
        /// The pack directory.
        dir: PathBuf,
        /// What roadshield rejected.
        #[source]
        source: roadshield::PackError,
    },
}

/// A shield cannot be made for a request.
#[derive(Debug, thiserror::Error)]
pub enum ShieldRenderError {
    /// The image name is not a route request.
    #[error("{name:?} is not a route request: {reason}")]
    Request {
        /// The image id after the namespace.
        name: String,
        /// What is wrong with it.
        reason: &'static str,
    },
    /// The engine rejected the route or failed to draw it.
    #[error("roadshield cannot draw {name:?}")]
    Shield {
        /// The image id after the namespace.
        name: String,
        /// The engine's error.
        #[source]
        source: roadshield::ShieldError,
    },
    /// The engine's SVG does not parse.
    #[error("the SVG of shield {key} does not parse")]
    Svg {
        /// The shield's semantic key.
        key: String,
        /// The parser's error.
        #[source]
        source: resvg::usvg::Error,
    },
    /// The shield would be larger than a label image may be.
    #[error("shield {key} would be {width} x {height} pixels")]
    TooLarge {
        /// The shield's semantic key.
        key: String,
        /// Width in device pixels.
        width: u32,
        /// Height in device pixels.
        height: u32,
    },
}
