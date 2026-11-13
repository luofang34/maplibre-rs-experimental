//! MBTiles path decoding, gzip/zlib decompression, and TileJSON 3.0 document synthesis.

use std::{collections::HashMap, io::Read, path::PathBuf};

use flate2::read::{GzDecoder, ZlibDecoder};
use rusqlite::Connection;

use super::{MbtilesFormatError, SCHEME};

pub(super) fn resolve_archive_path(archive: &str) -> PathBuf {
    let stripped = archive
        .strip_prefix("file://localhost")
        .or_else(|| archive.strip_prefix("file://"))
        .unwrap_or(archive);
    #[cfg(windows)]
    let stripped = if stripped.as_bytes().get(2) == Some(&b':') && stripped.starts_with('/') {
        &stripped[1..]
    } else {
        stripped
    };
    percent_decode_path(stripped)
}

fn percent_decode_path(input: &str) -> PathBuf {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0;
    while idx < bytes.len() {
        if bytes[idx] == b'%' && idx + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (
                (bytes[idx + 1] as char).to_digit(16),
                (bytes[idx + 2] as char).to_digit(16),
            ) {
                out.push(((hi << 4) | lo) as u8);
                idx += 3;
                continue;
            }
        }
        out.push(bytes[idx]);
        idx += 1;
    }
    PathBuf::from(String::from_utf8_lossy(&out).into_owned())
}

pub(super) fn decode_tile_payload(bytes: Vec<u8>) -> Result<Vec<u8>, MbtilesFormatError> {
    if is_raster_magic(&bytes) {
        return Ok(bytes);
    }
    if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut decoder = GzDecoder::new(bytes.as_slice());
        let mut out = Vec::new();
        decoder
            .read_to_end(&mut out)
            .map_err(|source| MbtilesFormatError::Decompress {
                encoding: "gzip",
                source,
            })?;
        return Ok(out);
    }
    if is_zlib_header(&bytes) {
        let mut decoder = ZlibDecoder::new(bytes.as_slice());
        let mut out = Vec::new();
        decoder
            .read_to_end(&mut out)
            .map_err(|source| MbtilesFormatError::Decompress {
                encoding: "zlib",
                source,
            })?;
        return Ok(out);
    }
    Ok(bytes)
}

fn is_raster_magic(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || bytes.starts_with(&[0xff, 0xd8, 0xff])
        || (bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP")
}

fn is_zlib_header(bytes: &[u8]) -> bool {
    if bytes.len() < 2 {
        return false;
    }
    let cmf = bytes[0];
    let flg = bytes[1];
    cmf == 0x78 && ((u16::from(cmf) << 8) | u16::from(flg)).is_multiple_of(31)
}

pub(super) fn build_tile_json(
    conn: &Connection,
    archive: &str,
) -> Result<Vec<u8>, MbtilesFormatError> {
    let mut rows_map: HashMap<String, String> = HashMap::new();
    if let Ok(mut stmt) = conn.prepare("SELECT name, value FROM metadata") {
        if let Ok(mapped) = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        }) {
            for pair in mapped.flatten() {
                rows_map.insert(pair.0, pair.1);
            }
        }
    }

    let mut doc = match rows_map
        .get("json")
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
    {
        Some(serde_json::Value::Object(fields)) => fields,
        _ => serde_json::Map::new(),
    };

    doc.insert("tilejson".to_owned(), serde_json::json!("3.0.0"));
    doc.insert(
        "tiles".to_owned(),
        serde_json::json!([format!("{SCHEME}{archive}/{{z}}/{{x}}/{{y}}")]),
    );
    doc.insert("scheme".to_owned(), serde_json::json!("xyz"));

    if let Some(minzoom) = rows_map
        .get("minzoom")
        .and_then(|v| v.trim().parse::<u8>().ok())
    {
        doc.insert("minzoom".to_owned(), serde_json::json!(minzoom));
    }
    if let Some(maxzoom) = rows_map
        .get("maxzoom")
        .and_then(|v| v.trim().parse::<u8>().ok())
    {
        doc.insert("maxzoom".to_owned(), serde_json::json!(maxzoom));
    }
    if let Some(bounds) = rows_map.get("bounds").and_then(|v| parse_f64_list(v, 4)) {
        doc.insert("bounds".to_owned(), serde_json::json!(bounds));
    }
    if let Some(center) = rows_map
        .get("center")
        .and_then(|v| parse_f64_list(v, 3).or_else(|| parse_f64_list(v, 2)))
    {
        doc.insert("center".to_owned(), serde_json::json!(center));
    }
    if let Some(tile_size) = rows_map
        .get("tileSize")
        .or_else(|| rows_map.get("tilesize"))
        .and_then(|v| v.trim().parse::<u32>().ok())
    {
        doc.insert("tileSize".to_owned(), serde_json::json!(tile_size));
    }
    if let Some(encoding) = rows_map.get("encoding") {
        let normalized = encoding.trim().to_ascii_lowercase();
        if matches!(normalized.as_str(), "mapbox" | "terrarium") {
            doc.insert("encoding".to_owned(), serde_json::json!(normalized));
        }
    }
    for key in [
        "name",
        "description",
        "version",
        "attribution",
        "format",
        "type",
    ] {
        if let Some(val) = rows_map.get(key) {
            doc.insert(key.to_owned(), serde_json::json!(val));
        }
    }

    Ok(serde_json::to_vec(&doc)?)
}

fn parse_f64_list(raw: &str, expected_len: usize) -> Option<Vec<f64>> {
    let parsed: Option<Vec<f64>> = raw
        .split(',')
        .map(|part| part.trim().parse::<f64>().ok())
        .collect();
    parsed.filter(|items| items.len() == expected_len)
}
