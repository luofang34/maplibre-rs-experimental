//! The PMTiles v3 layout: a fixed header, varint-encoded directories, and tile IDs that
//! number tiles zoom by zoom along a Hilbert curve.

use std::io::Read;

use thiserror::Error;

/// Bytes of the fixed header at the start of an archive.
pub(super) const HEADER_BYTES: u64 = 127;

/// An archive's bytes do not follow the PMTiles v3 layout this reader understands.
#[derive(Error, Debug)]
pub enum PmtilesFormatError {
    /// The archive does not start with the PMTiles v3 magic.
    #[error("not a PMTiles v3 archive")]
    NotPmtiles,
    /// The archive is shorter than its header says.
    #[error("the archive ends inside its {0}")]
    Truncated(&'static str),
    /// Data is compressed with a scheme other than none or gzip.
    #[error("unsupported compression {0}")]
    Compression(u8),
    /// Compressed data does not decompress.
    #[error("cannot decompress {what}")]
    Decompress {
        /// What was being decompressed.
        what: &'static str,
        /// The decoder's error.
        #[source]
        source: std::io::Error,
    },
    /// Leaf directories nest deeper than the format allows.
    #[error("leaf directories nest too deep")]
    TooDeep,
    /// The tile coordinates have no tile ID.
    #[error("zoom {0} is past the deepest PMTiles zoom")]
    Zoom(u8),
}

/// The fixed header of an archive.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Header {
    /// Where the root directory starts and how long it is.
    pub root: (u64, u64),
    /// Where the JSON metadata starts and how long it is.
    pub metadata: (u64, u64),
    /// Where leaf directories start.
    pub leaves_offset: u64,
    /// Where tile data starts.
    pub tiles_offset: u64,
    /// Compression of directories and metadata.
    pub internal_compression: u8,
    /// Compression of each tile.
    pub tile_compression: u8,
    /// Kind of tile: 1 vector, 2 PNG, 3 JPEG, 4 WebP, 5 AVIF.
    pub tile_type: u8,
    /// Zoom range of the tiles.
    pub zooms: (u8, u8),
    /// West, south, east and north edges in degrees.
    pub bounds: [f64; 4],
    /// Longitude, latitude and zoom of the initial view.
    pub center: (f64, f64, u8),
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    let mut word = [0; 8];
    word.copy_from_slice(&bytes[at..at + 8]);
    u64::from_le_bytes(word)
}

fn degrees_at(bytes: &[u8], at: usize) -> f64 {
    let mut word = [0; 4];
    word.copy_from_slice(&bytes[at..at + 4]);
    f64::from(i32::from_le_bytes(word)) / 1e7
}

impl Header {
    /// Reads the header from the first bytes of an archive.
    pub fn parse(bytes: &[u8]) -> Result<Self, PmtilesFormatError> {
        if bytes.len() < HEADER_BYTES as usize {
            return Err(PmtilesFormatError::Truncated("header"));
        }
        if &bytes[..7] != b"PMTiles" || bytes[7] != 3 {
            return Err(PmtilesFormatError::NotPmtiles);
        }
        Ok(Self {
            root: (u64_at(bytes, 8), u64_at(bytes, 16)),
            metadata: (u64_at(bytes, 24), u64_at(bytes, 32)),
            leaves_offset: u64_at(bytes, 40),
            tiles_offset: u64_at(bytes, 56),
            internal_compression: bytes[97],
            tile_compression: bytes[98],
            tile_type: bytes[99],
            zooms: (bytes[100], bytes[101]),
            bounds: [
                degrees_at(bytes, 102),
                degrees_at(bytes, 106),
                degrees_at(bytes, 110),
                degrees_at(bytes, 114),
            ],
            center: (degrees_at(bytes, 119), degrees_at(bytes, 123), bytes[118]),
        })
    }
}

/// Undoes `compression` (1 none, 2 gzip).
pub fn decompress(
    compression: u8,
    bytes: Vec<u8>,
    what: &'static str,
) -> Result<Vec<u8>, PmtilesFormatError> {
    match compression {
        // 0 is "unknown", which writers use for data they did not compress.
        0 | 1 => Ok(bytes),
        2 => {
            let mut out = Vec::new();
            flate2::read::GzDecoder::new(bytes.as_slice())
                .read_to_end(&mut out)
                .map_err(|source| PmtilesFormatError::Decompress { what, source })?;
            Ok(out)
        }
        other => Err(PmtilesFormatError::Compression(other)),
    }
}

/// One directory entry: a run of tiles stored at one place, or with no run a leaf directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    /// First tile ID of the run.
    pub tile_id: u64,
    /// Offset from the tile data, or from the leaf directories for a leaf.
    pub offset: u64,
    /// Bytes stored.
    pub length: u64,
    /// Consecutive tile IDs sharing these bytes; zero for a leaf directory.
    pub run_length: u64,
}

fn varint(bytes: &[u8], at: &mut usize) -> Result<u64, PmtilesFormatError> {
    let mut value = 0_u64;
    for shift in (0..64).step_by(7) {
        let byte = *bytes
            .get(*at)
            .ok_or(PmtilesFormatError::Truncated("directory"))?;
        *at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(PmtilesFormatError::Truncated("directory"))
}

/// Decodes a decompressed directory: its entry count, then each column of the entries.
pub fn parse_directory(bytes: &[u8]) -> Result<Vec<Entry>, PmtilesFormatError> {
    let mut at = 0;
    let count = usize::try_from(varint(bytes, &mut at)?)
        .map_err(|_| PmtilesFormatError::Truncated("directory"))?;
    // Each entry takes at least four bytes, which bounds a corrupt count.
    if count > bytes.len() {
        return Err(PmtilesFormatError::Truncated("directory"));
    }
    let mut entries = vec![
        Entry {
            tile_id: 0,
            offset: 0,
            length: 0,
            run_length: 0,
        };
        count
    ];
    let mut tile_id = 0_u64;
    for entry in &mut entries {
        tile_id = tile_id.wrapping_add(varint(bytes, &mut at)?);
        entry.tile_id = tile_id;
    }
    for entry in &mut entries {
        entry.run_length = varint(bytes, &mut at)?;
    }
    for entry in &mut entries {
        entry.length = varint(bytes, &mut at)?;
    }
    for index in 0..count {
        let value = varint(bytes, &mut at)?;
        entries[index].offset = match (value, index.checked_sub(1)) {
            // Zero continues right after the previous entry's bytes.
            (0, Some(previous)) => entries[previous].offset + entries[previous].length,
            (value, _) => value.saturating_sub(1),
        };
    }
    Ok(entries)
}

/// Where a tile ID leads within a directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Found {
    /// The tile's bytes, relative to the tile data.
    Tile(Entry),
    /// A leaf directory to look in, relative to the leaf directories.
    Leaf(Entry),
}

/// Looks `tile_id` up among `entries`, which are sorted by tile ID.
pub fn find(entries: &[Entry], tile_id: u64) -> Option<Found> {
    let index = entries
        .partition_point(|entry| entry.tile_id <= tile_id)
        .checked_sub(1)?;
    let entry = entries[index];
    if entry.run_length == 0 {
        return Some(Found::Leaf(entry));
    }
    (tile_id - entry.tile_id < entry.run_length).then_some(Found::Tile(entry))
}

/// The tile ID of `(z, x, y)`: every tile of the zooms above, then the tile's place along
/// the Hilbert curve through its zoom.
pub fn tile_id(z: u8, x: u32, y: u32) -> Result<u64, PmtilesFormatError> {
    if z > 31 {
        return Err(PmtilesFormatError::Zoom(z));
    }
    let above = ((1_u128 << (2 * u32::from(z))) - 1) / 3;
    let (mut x, mut y) = (u64::from(x), u64::from(y));
    let mut d = 0_u64;
    let mut s = (1_u64 << z) / 2;
    while s > 0 {
        let rx = u64::from(x & s > 0);
        let ry = u64::from(y & s > 0);
        d += s * s * ((3 * rx) ^ ry);
        // Only the bits below `s` matter to the steps that follow.
        if ry == 0 {
            if rx == 1 {
                x = s - 1 - (x & (s - 1));
                y = s - 1 - (y & (s - 1));
            }
            std::mem::swap(&mut x, &mut y);
        }
        s /= 2;
    }
    u64::try_from(above)
        .ok()
        .and_then(|above| above.checked_add(d))
        .ok_or(PmtilesFormatError::Zoom(z))
}

#[cfg(test)]
mod tests;
