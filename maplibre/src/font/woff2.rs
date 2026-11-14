//! Pure-Rust WOFF 2.0 (`wOF2`) Brotli decompression and OpenType `sfnt` table reconstruction.

use std::io::Read;

use brotli_decompressor::Decompressor;

use super::{
    container::{assemble_sfnt, SfntTable, MAX_DECOMPRESSED_FONT_BYTES},
    FontError,
};

mod glyf_reconstruct;

const WOFF2_HEADER_SIZE: usize = 48;

/// Standard 63 known table tags defined in W3C WOFF File Format 2.0, Section 3.2 (Table 1).
const KNOWN_TAGS: [[u8; 4]; 63] = [
    *b"cmap", *b"head", *b"hhea", *b"hmtx", *b"maxp", *b"name", *b"OS/2", *b"post", *b"cvt ",
    *b"fpgm", *b"glyf", *b"loca", *b"prep", *b"CFF ", *b"VORG", *b"EBDT", *b"EBLC", *b"gasp",
    *b"hdmx", *b"kern", *b"LTSH", *b"PCLT", *b"VDMX", *b"vhea", *b"vmtx", *b"BASE", *b"GDEF",
    *b"GPOS", *b"GSUB", *b"EBSC", *b"JSTF", *b"MATH", *b"CBDT", *b"CBLC", *b"COLR", *b"CPAL",
    *b"SVG ", *b"sbix", *b"acnt", *b"avar", *b"bdat", *b"bloc", *b"bsln", *b"cvar", *b"fdsc",
    *b"feat", *b"fmtx", *b"fvar", *b"gvar", *b"hsty", *b"just", *b"lcar", *b"mort", *b"morx",
    *b"opbd", *b"prop", *b"trak", *b"Zapf", *b"Silf", *b"Glat", *b"Gloc", *b"Feat", *b"Sill",
];

/// Bounds-checked big-endian byte cursor used across WOFF2 header and substream decoders.
pub(super) struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    pub(super) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    pub(super) fn position(&self) -> usize {
        self.pos
    }

    pub(super) fn read_u8(&mut self) -> Result<u8, FontError> {
        let &byte = self
            .bytes
            .get(self.pos)
            .ok_or(FontError::InvalidContainer("WOFF2 unexpected EOF"))?;
        self.pos += 1;
        Ok(byte)
    }

    pub(super) fn read_slice(&mut self, len: usize) -> Result<&'a [u8], FontError> {
        let end = self
            .pos
            .checked_add(len)
            .filter(|&end| end <= self.bytes.len())
            .ok_or(FontError::InvalidContainer("WOFF2 slice out of bounds"))?;
        let slice = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    pub(super) fn read_u16_be(&mut self) -> Result<u16, FontError> {
        let s = self.read_slice(2)?;
        Ok(u16::from_be_bytes([s[0], s[1]]))
    }

    pub(super) fn read_i16_be(&mut self) -> Result<i16, FontError> {
        let s = self.read_slice(2)?;
        Ok(i16::from_be_bytes([s[0], s[1]]))
    }

    pub(super) fn read_u32_be(&mut self) -> Result<u32, FontError> {
        let s = self.read_slice(4)?;
        Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }

    /// Reads a WOFF2 `UIntBase128` variable-length integer (W3C WOFF 2.0 Section 2).
    pub(super) fn read_uint_base128(&mut self) -> Result<u32, FontError> {
        let mut accum = 0_u32;
        for idx in 0..5 {
            let byte = self.read_u8()?;
            if idx == 0 && byte == 0x80 {
                return Err(FontError::InvalidContainer(
                    "WOFF2 UIntBase128 leading zero",
                ));
            }
            if (accum & 0xFE00_0000) != 0 {
                return Err(FontError::InvalidContainer("WOFF2 UIntBase128 overflow"));
            }
            accum = (accum << 7) | u32::from(byte & 0x7F);
            if (byte & 0x80) == 0 {
                return Ok(accum);
            }
        }
        Err(FontError::InvalidContainer(
            "WOFF2 UIntBase128 exceeds 5 bytes",
        ))
    }

    /// Reads a WOFF2 `255UShort` variable-length integer (W3C WOFF 2.0 Section 2).
    pub(super) fn read_255_ushort(&mut self) -> Result<u16, FontError> {
        let code = self.read_u8()?;
        match code {
            253 => self.read_u16_be(),
            255 => Ok(253_u16.saturating_add(u16::from(self.read_u8()?))),
            254 => Ok(506_u16.saturating_add(u16::from(self.read_u8()?))),
            value => Ok(u16::from(value)),
        }
    }
}

struct Woff2DirEntry {
    tag: [u8; 4],
    transformed: bool,
    stream_offset: usize,
    stream_len: usize,
}

/// Unpacks a WOFF 2.0 (`wOF2`) buffer into a standard OpenType/TrueType `sfnt` byte buffer.
pub(super) fn unpack_woff2(bytes: &[u8]) -> Result<Vec<u8>, FontError> {
    if bytes.len() < WOFF2_HEADER_SIZE || bytes.get(..4) != Some(b"wOF2") {
        return Err(FontError::InvalidContainer(
            "truncated or invalid WOFF2 header",
        ));
    }
    let mut cursor = Cursor::new(bytes);
    let _sig = cursor.read_u32_be()?;
    let mut flavor = cursor.read_u32_be()?;
    let declared_len = cursor.read_u32_be()? as usize;
    if declared_len > bytes.len() {
        return Err(FontError::InvalidContainer("WOFF2 length exceeds buffer"));
    }
    let num_tables = cursor.read_u16_be()? as usize;
    let reserved = cursor.read_u16_be()?;
    if num_tables == 0 || reserved != 0 {
        return Err(FontError::InvalidContainer(
            "WOFF2 header has zero tables or non-zero reserved field",
        ));
    }
    let _total_sfnt_size = cursor.read_u32_be()?;
    let total_compressed_size = cursor.read_u32_be()? as usize;
    // Skip majorVersion, minorVersion, metaOffset/Length/OrigLength, privOffset/Length (24 bytes).
    let _ = cursor.read_slice(24)?;

    let mut entries = Vec::with_capacity(num_tables);
    let mut stream_total = 0usize;
    for _ in 0..num_tables {
        let flags = cursor.read_u8()?;
        let tag_index = usize::from(flags & 0x3F);
        let tag = if tag_index == 63 {
            let s = cursor.read_slice(4)?;
            [s[0], s[1], s[2], s[3]]
        } else {
            KNOWN_TAGS[tag_index]
        };
        let xform_version = (flags >> 6) & 0x03;
        let orig_length = cursor.read_uint_base128()? as usize;
        let is_glyf_or_loca = &tag == b"glyf" || &tag == b"loca";
        let transformed = if is_glyf_or_loca {
            xform_version == 0
        } else {
            xform_version != 0
        };
        let stream_len = if transformed {
            cursor.read_uint_base128()? as usize
        } else {
            orig_length
        };
        if &tag == b"loca" && transformed && stream_len != 0 {
            return Err(FontError::InvalidContainer(
                "WOFF2 transformed loca must have zero stream length",
            ));
        }
        let stream_offset = stream_total;
        stream_total = stream_total
            .checked_add(stream_len)
            .filter(|&sum| sum <= MAX_DECOMPRESSED_FONT_BYTES)
            .ok_or(FontError::InvalidContainer(
                "WOFF2 uncompressed stream too large",
            ))?;
        entries.push(Woff2DirEntry {
            tag,
            transformed,
            stream_offset,
            stream_len,
        });
    }

    // Optional TTC collection directory (`ttcf` flavor): select the first face's flavor & tables.
    let mut selected_indices: Option<Vec<usize>> = None;
    if &flavor.to_be_bytes() == b"ttcf" {
        let _ttc_version = cursor.read_u32_be()?;
        let num_fonts = cursor.read_255_ushort()? as usize;
        if num_fonts == 0 {
            return Err(FontError::InvalidContainer(
                "WOFF2 collection has zero fonts",
            ));
        }
        for font_idx in 0..num_fonts {
            let count = cursor.read_255_ushort()? as usize;
            let face_flavor = cursor.read_u32_be()?;
            let mut indices = Vec::with_capacity(count);
            for _ in 0..count {
                let idx = cursor.read_255_ushort()? as usize;
                if idx >= entries.len() {
                    return Err(FontError::InvalidContainer(
                        "WOFF2 collection table index out of bounds",
                    ));
                }
                indices.push(idx);
            }
            if font_idx == 0 {
                flavor = face_flavor;
                selected_indices = Some(indices);
            }
        }
    }

    let compressed = cursor.read_slice(total_compressed_size)?;
    let mut stream = Vec::with_capacity(stream_total.min(131_072));
    let limit = u64::try_from(stream_total)
        .map_err(|_| FontError::InvalidContainer("WOFF2 stream too large"))?
        .saturating_add(1);
    Decompressor::new(compressed, 4096)
        .take(limit)
        .read_to_end(&mut stream)
        .map_err(|_| FontError::InvalidContainer("WOFF2 Brotli decompression failed"))?;
    if stream.len() < stream_total {
        return Err(FontError::InvalidContainer(
            "WOFF2 decompressed stream shorter than declared tables",
        ));
    }

    let active_entries: Vec<&Woff2DirEntry> = match &selected_indices {
        Some(indices) => indices.iter().map(|&idx| &entries[idx]).collect(),
        None => entries.iter().collect(),
    };

    let mut raw_tables = Vec::with_capacity(active_entries.len());
    let mut reconstructed_glyf_loca: Option<glyf_reconstruct::ReconstructedGlyf> = None;

    for entry in &active_entries {
        if &entry.tag == b"glyf" && entry.transformed {
            let slice = &stream[entry.stream_offset..entry.stream_offset + entry.stream_len];
            reconstructed_glyf_loca = Some(glyf_reconstruct::reconstruct_glyf_loca(slice)?);
        }
    }

    // Locate `hhea` and `maxp` slices in case `hmtx` uses transform version 1.
    let find_raw = |target: &[u8; 4]| -> Option<&[u8]> {
        active_entries
            .iter()
            .find(|e| &e.tag == target && !e.transformed)
            .map(|e| &stream[e.stream_offset..e.stream_offset + e.stream_len])
    };
    let hhea_slice = find_raw(b"hhea");
    let maxp_slice = find_raw(b"maxp");

    for entry in &active_entries {
        let data = match (&entry.tag, entry.transformed) {
            (b"glyf", true) => reconstructed_glyf_loca
                .as_ref()
                .map(|r| r.glyf.clone())
                .ok_or(FontError::InvalidContainer("missing reconstructed glyf"))?,
            (b"loca", true) => reconstructed_glyf_loca
                .as_ref()
                .map(|r| r.loca.clone())
                .ok_or(FontError::InvalidContainer("missing reconstructed loca"))?,
            (b"hmtx", true) => {
                let slice = &stream[entry.stream_offset..entry.stream_offset + entry.stream_len];
                let x_mins = reconstructed_glyf_loca
                    .as_ref()
                    .map(|r| r.x_mins.as_slice())
                    .unwrap_or(&[]);
                glyf_reconstruct::reconstruct_hmtx(slice, hhea_slice, maxp_slice, x_mins)?
            }
            (_, true) => {
                return Err(FontError::InvalidContainer(
                    "unsupported WOFF2 table transform",
                ))
            }
            (b"head", false) => {
                let mut head =
                    stream[entry.stream_offset..entry.stream_offset + entry.stream_len].to_vec();
                if let Some(reconstructed) = &reconstructed_glyf_loca {
                    if head.len() >= 52 {
                        head[50..52].copy_from_slice(&reconstructed.index_format.to_be_bytes());
                    }
                }
                head
            }
            (_, false) => {
                stream[entry.stream_offset..entry.stream_offset + entry.stream_len].to_vec()
            }
        };
        raw_tables.push(SfntTable {
            tag: entry.tag,
            data,
        });
    }

    assemble_sfnt(flavor, raw_tables)
}
