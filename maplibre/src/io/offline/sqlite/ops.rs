//! SQLite schema, zlib resource compression, quota enforcement, and sideload database merge helpers.

use std::{
    collections::HashMap,
    io::{Read, Write},
    path::Path,
    sync::{
        atomic::{AtomicI64, Ordering},
        Mutex, PoisonError,
    },
};

use flate2::{read::ZlibDecoder, write::ZlibEncoder, Compression};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};

use crate::io::{
    offline::store::{OfflineRegionId, OfflineRegionStatus, OfflineStoreError},
    source_client::ResourceKind,
};

pub(super) const DEFAULT_AMBIENT_BYTES: i64 = 32 * 1024 * 1024;
pub(super) const CFG_MAX_AMBIENT_BYTES: &str = "max_ambient_bytes";
pub(super) const CFG_TILE_COUNT_LIMIT: &str = "tile_count_limit";

pub(super) const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS offline_config (
    key TEXT PRIMARY KEY,
    value INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS regions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    definition_json TEXT NOT NULL,
    metadata BLOB NOT NULL,
    complete INTEGER NOT NULL DEFAULT 0,
    required_count INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS resources (
    url TEXT PRIMARY KEY,
    kind INTEGER NOT NULL,
    data BLOB NOT NULL,
    compressed INTEGER NOT NULL DEFAULT 0,
    raw_size INTEGER NOT NULL,
    stored_size INTEGER NOT NULL,
    ambient INTEGER NOT NULL DEFAULT 0,
    accessed_at INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS region_resources (
    region_id INTEGER NOT NULL,
    url TEXT NOT NULL,
    PRIMARY KEY (region_id, url)
);
CREATE INDEX IF NOT EXISTS idx_region_resources_url ON region_resources (url);
";

pub(super) struct SqliteInner {
    pub(super) conn: Mutex<Connection>,
    pub(super) clock: AtomicI64,
}

impl SqliteInner {
    pub(super) fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn next_stamp(&self) -> i64 {
        self.clock.fetch_add(1, Ordering::Relaxed) + 1
    }
}

pub(super) fn sql_err(err: rusqlite::Error) -> OfflineStoreError {
    OfflineStoreError::Backend(err.to_string())
}

pub(super) fn kind_to_i32(kind: ResourceKind) -> i32 {
    match kind {
        ResourceKind::Style => 1,
        ResourceKind::Source => 2,
        ResourceKind::Tile => 3,
        ResourceKind::Glyphs => 4,
        ResourceKind::SpriteImage => 5,
        ResourceKind::SpriteJson => 6,
        ResourceKind::Image => 7,
        ResourceKind::Unknown => 0,
    }
}

pub(super) fn encode_resource_blob(raw: &[u8]) -> (Vec<u8>, bool) {
    let already_compressed_image = raw.starts_with(b"\x89PNG\r\n\x1a\n")
        || raw.starts_with(&[0xff, 0xd8, 0xff])
        || (raw.len() >= 12 && &raw[0..4] == b"RIFF" && &raw[8..12] == b"WEBP");
    if raw.len() < 128 || already_compressed_image {
        return (raw.to_vec(), false);
    }
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    if encoder.write_all(raw).is_ok() {
        if let Ok(compressed) = encoder.finish() {
            if compressed.len() < raw.len() {
                return (compressed, true);
            }
        }
    }
    (raw.to_vec(), false)
}

pub(super) fn decode_resource_blob(
    stored: Vec<u8>,
    compressed: bool,
) -> Result<Vec<u8>, OfflineStoreError> {
    if !compressed {
        return Ok(stored);
    }
    let mut decoder = ZlibDecoder::new(stored.as_slice());
    let mut out = Vec::new();
    decoder.read_to_end(&mut out)?;
    Ok(out)
}

pub(super) fn read_tile_limit(conn: &Connection) -> Result<Option<u64>, OfflineStoreError> {
    let val: Option<i64> = conn
        .query_row(
            "SELECT value FROM offline_config WHERE key = ?1",
            params![CFG_TILE_COUNT_LIMIT],
            |row| row.get(0),
        )
        .optional()
        .map_err(sql_err)?;
    Ok(val.and_then(|v| (v >= 0).then_some(v as u64)))
}

pub(super) fn read_max_ambient_bytes(conn: &Connection) -> Result<u64, OfflineStoreError> {
    let val: Option<i64> = conn
        .query_row(
            "SELECT value FROM offline_config WHERE key = ?1",
            params![CFG_MAX_AMBIENT_BYTES],
            |row| row.get(0),
        )
        .optional()
        .map_err(sql_err)?;
    Ok(val.unwrap_or(DEFAULT_AMBIENT_BYTES).max(0) as u64)
}

pub(super) fn pinned_tile_count(conn: &Connection) -> Result<u64, OfflineStoreError> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM resources r
             WHERE r.kind = ?1
               AND EXISTS (SELECT 1 FROM region_resources rr WHERE rr.url = r.url)",
            params![kind_to_i32(ResourceKind::Tile)],
            |row| row.get(0),
        )
        .map_err(sql_err)?;
    Ok(count.max(0) as u64)
}

pub(super) fn is_url_pinned(conn: &Connection, url: &str) -> Result<bool, OfflineStoreError> {
    let exists: Option<i32> = conn
        .query_row(
            "SELECT 1 FROM region_resources WHERE url = ?1 LIMIT 1",
            params![url],
            |row| row.get(0),
        )
        .optional()
        .map_err(sql_err)?;
    Ok(exists.is_some())
}

pub(super) fn gc_orphaned_non_ambient(conn: &Connection) -> Result<(), OfflineStoreError> {
    conn.execute(
        "DELETE FROM resources
         WHERE ambient = 0
           AND NOT EXISTS (SELECT 1 FROM region_resources rr WHERE rr.url = resources.url)",
        [],
    )
    .map_err(sql_err)?;
    Ok(())
}

pub(super) fn enforce_ambient_budget(conn: &Connection) -> Result<(), OfflineStoreError> {
    let budget = read_max_ambient_bytes(conn)?;
    let total: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(stored_size), 0) FROM resources r
             WHERE r.ambient = 1
               AND NOT EXISTS (SELECT 1 FROM region_resources rr WHERE rr.url = r.url)",
            [],
            |row| row.get(0),
        )
        .map_err(sql_err)?;
    let mut current = total.max(0) as u64;
    if current <= budget {
        return Ok(());
    }
    let mut stmt = conn
        .prepare(
            "SELECT r.url, r.stored_size FROM resources r
             WHERE r.ambient = 1
               AND NOT EXISTS (SELECT 1 FROM region_resources rr WHERE rr.url = r.url)
             ORDER BY r.accessed_at ASC",
        )
        .map_err(sql_err)?;
    let candidates: Vec<(String, i64)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(sql_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql_err)?;
    drop(stmt);

    for (url, sz) in candidates {
        if current <= budget {
            break;
        }
        conn.execute("DELETE FROM resources WHERE url = ?1", params![url])
            .map_err(sql_err)?;
        current = current.saturating_sub(sz.max(0) as u64);
    }
    Ok(())
}

pub(super) fn query_region_status(
    conn: &Connection,
    id: OfflineRegionId,
) -> Result<OfflineRegionStatus, OfflineStoreError> {
    let (complete, required): (i64, i64) = conn
        .query_row(
            "SELECT complete, required_count FROM regions WHERE id = ?1",
            params![id.0],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(sql_err)?
        .ok_or(OfflineStoreError::RegionNotFound(id))?;

    let (res_count, res_bytes, tile_count): (i64, i64, i64) = conn
        .query_row(
            "SELECT
                COUNT(r.url),
                COALESCE(SUM(r.raw_size), 0),
                COALESCE(SUM(CASE WHEN r.kind = ?2 THEN 1 ELSE 0 END), 0)
             FROM region_resources rr
             JOIN resources r ON r.url = rr.url
             WHERE rr.region_id = ?1",
            params![id.0, kind_to_i32(ResourceKind::Tile)],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(sql_err)?;

    Ok(OfflineRegionStatus {
        complete: complete != 0,
        completed_resource_count: res_count.max(0) as u64,
        completed_resource_bytes: res_bytes.max(0) as u64,
        completed_tile_count: tile_count.max(0) as u64,
        required_resource_count: required.max(0) as u64,
    })
}

pub(super) fn merge_database_sync(
    inner: &SqliteInner,
    sideloaded_path: &Path,
) -> Result<Vec<OfflineRegionId>, OfflineStoreError> {
    let src = Connection::open_with_flags(
        sideloaded_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(sql_err)?;

    type SrcRegionRow = (i64, String, Vec<u8>, i64, i64);
    let src_regions: Vec<SrcRegionRow> = src
        .prepare("SELECT id, definition_json, metadata, complete, required_count FROM regions ORDER BY id ASC")
        .map_err(sql_err)?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
        .map_err(sql_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql_err)?;

    type SrcResourceRow = (String, i32, Vec<u8>, i64, i64, i64, i64);
    let src_resources: Vec<SrcResourceRow> = src
        .prepare(
            "SELECT url, kind, data, compressed, raw_size, stored_size, ambient FROM resources",
        )
        .map_err(sql_err)?
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
            ))
        })
        .map_err(sql_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql_err)?;

    let src_links: Vec<(i64, String)> = src
        .prepare("SELECT region_id, url FROM region_resources")
        .map_err(sql_err)?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(sql_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql_err)?;

    let mut dst = inner.lock();
    let tx = dst.transaction().map_err(sql_err)?;

    if let Some(limit) = read_tile_limit(&tx)? {
        let current_tiles = pinned_tile_count(&tx)?;
        let tile_kind = kind_to_i32(ResourceKind::Tile);
        let mut newly_pinned_tiles = 0_u64;
        for (url, kind, ..) in &src_resources {
            let linked_in_src = src_links.iter().any(|(_, u)| u == url);
            if *kind == tile_kind && linked_in_src && !is_url_pinned(&tx, url)? {
                newly_pinned_tiles += 1;
            }
        }
        if current_tiles.saturating_add(newly_pinned_tiles) > limit {
            return Err(OfflineStoreError::TileLimitExceeded { limit });
        }
    }

    for (url, kind, data, compressed, raw_size, stored_size, ambient) in src_resources {
        let stamp = inner.next_stamp();
        tx.execute(
            "INSERT INTO resources (url, kind, data, compressed, raw_size, stored_size, ambient, accessed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(url) DO UPDATE SET
                kind = excluded.kind,
                data = excluded.data,
                compressed = excluded.compressed,
                raw_size = excluded.raw_size,
                stored_size = excluded.stored_size,
                accessed_at = excluded.accessed_at",
            params![url, kind, data, compressed, raw_size, stored_size, ambient, stamp],
        )
        .map_err(sql_err)?;
    }

    let mut id_map: HashMap<i64, OfflineRegionId> = HashMap::new();
    let mut created_ids = Vec::with_capacity(src_regions.len());
    for (old_id, def_json, metadata, complete, req_count) in src_regions {
        tx.execute(
            "INSERT INTO regions (definition_json, metadata, complete, required_count)
             VALUES (?1, ?2, ?3, ?4)",
            params![def_json, metadata, complete, req_count],
        )
        .map_err(sql_err)?;
        let new_id = OfflineRegionId(tx.last_insert_rowid());
        id_map.insert(old_id, new_id);
        created_ids.push(new_id);
    }

    for (old_reg_id, url) in src_links {
        if let Some(new_id) = id_map.get(&old_reg_id) {
            tx.execute(
                "INSERT OR IGNORE INTO region_resources (region_id, url) VALUES (?1, ?2)",
                params![new_id.0, url],
            )
            .map_err(sql_err)?;
        }
    }

    enforce_ambient_budget(&tx)?;
    tx.commit().map_err(sql_err)?;
    Ok(created_ids)
}
