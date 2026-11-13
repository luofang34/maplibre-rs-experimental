//! Persistent SQLite [`OfflineStore`] implementation with tile deduplication, compression,
//! ambient eviction, tile limits, and sideloaded database merging.

mod ops;

use std::{
    path::{Path, PathBuf},
    sync::{atomic::AtomicI64, Arc, Mutex},
};

use async_trait::async_trait;
use rusqlite::{params, Connection, OptionalExtension};

use self::ops::{
    decode_resource_blob, encode_resource_blob, enforce_ambient_budget, gc_orphaned_non_ambient,
    is_url_pinned, kind_to_i32, merge_database_sync, pinned_tile_count, query_region_status,
    read_max_ambient_bytes, read_tile_limit, sql_err, SqliteInner, CFG_MAX_AMBIENT_BYTES,
    CFG_TILE_COUNT_LIMIT, DEFAULT_AMBIENT_BYTES, SCHEMA_SQL,
};
use super::{
    region::OfflineRegionDefinition,
    store::{
        OfflineRegionId, OfflineRegionStatus, OfflineRegionSummary, OfflineStore, OfflineStoreError,
    },
};
use crate::io::{cpu_work::run_cpu_bound, source_client::ResourceKind};

/// Persistent SQLite offline database supporting shared tile deduplication across regions,
/// transparent zlib compression, ambient cache eviction, and sideloaded database merging.
#[derive(Clone)]
pub struct SqliteOfflineStore {
    inner: Arc<SqliteInner>,
}

impl SqliteOfflineStore {
    /// Opens or creates an offline SQLite database at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, OfflineStoreError> {
        let conn = Connection::open(path.as_ref()).map_err(sql_err)?;
        let _ = conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;");
        Self::from_connection(conn)
    }

    /// Opens an isolated in-memory SQLite offline database.
    pub fn open_in_memory() -> Result<Self, OfflineStoreError> {
        let conn = Connection::open_in_memory().map_err(sql_err)?;
        Self::from_connection(conn)
    }

    fn from_connection(conn: Connection) -> Result<Self, OfflineStoreError> {
        conn.execute_batch(SCHEMA_SQL).map_err(sql_err)?;
        conn.execute(
            "INSERT OR IGNORE INTO offline_config (key, value) VALUES (?1, ?2)",
            params![CFG_MAX_AMBIENT_BYTES, DEFAULT_AMBIENT_BYTES],
        )
        .map_err(sql_err)?;
        let max_access: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(accessed_at), 0) FROM resources",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        Ok(Self {
            inner: Arc::new(SqliteInner {
                conn: Mutex::new(conn),
                clock: AtomicI64::new(max_access),
            }),
        })
    }

    /// Merges all regions and deduplicated resources from a sideloaded SQLite offline database
    /// at `sideloaded_path`, returning the newly assigned [`OfflineRegionId`]s.
    pub async fn merge_database(
        &self,
        sideloaded_path: impl AsRef<Path>,
    ) -> Result<Vec<OfflineRegionId>, OfflineStoreError> {
        let path: PathBuf = sideloaded_path.as_ref().to_path_buf();
        let inner = Arc::clone(&self.inner);
        run_cpu_bound(move || merge_database_sync(&inner, &path)).await?
    }
}

#[cfg_attr(not(feature = "thread-safe-futures"), async_trait(?Send))]
#[cfg_attr(feature = "thread-safe-futures", async_trait)]
impl OfflineStore for SqliteOfflineStore {
    async fn create_region(
        &self,
        definition: &OfflineRegionDefinition,
    ) -> Result<OfflineRegionId, OfflineStoreError> {
        let def_json = serde_json::to_string(definition)?;
        let meta = definition.metadata.clone();
        let inner = Arc::clone(&self.inner);
        run_cpu_bound(move || {
            let conn = inner.lock();
            conn.execute(
                "INSERT INTO regions (definition_json, metadata, complete, required_count)
                 VALUES (?1, ?2, 0, 0)",
                params![def_json, meta],
            )
            .map_err(sql_err)?;
            Ok(OfflineRegionId(conn.last_insert_rowid()))
        })
        .await?
    }

    async fn list_regions(&self) -> Result<Vec<OfflineRegionSummary>, OfflineStoreError> {
        let inner = Arc::clone(&self.inner);
        run_cpu_bound(move || {
            let conn = inner.lock();
            let mut stmt = conn
                .prepare("SELECT id, definition_json FROM regions ORDER BY id ASC")
                .map_err(sql_err)?;
            let rows: Vec<(i64, String)> = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .map_err(sql_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(sql_err)?;
            drop(stmt);

            let mut out = Vec::with_capacity(rows.len());
            for (raw_id, def_json) in rows {
                let id = OfflineRegionId(raw_id);
                let definition: OfflineRegionDefinition = serde_json::from_str(&def_json)?;
                let status = query_region_status(&conn, id)?;
                out.push(OfflineRegionSummary {
                    id,
                    definition,
                    status,
                });
            }
            Ok(out)
        })
        .await?
    }

    async fn region_definition(
        &self,
        id: OfflineRegionId,
    ) -> Result<OfflineRegionDefinition, OfflineStoreError> {
        let inner = Arc::clone(&self.inner);
        run_cpu_bound(move || {
            let conn = inner.lock();
            let raw: String = conn
                .query_row(
                    "SELECT definition_json FROM regions WHERE id = ?1",
                    params![id.0],
                    |r| r.get(0),
                )
                .optional()
                .map_err(sql_err)?
                .ok_or(OfflineStoreError::RegionNotFound(id))?;
            Ok(serde_json::from_str(&raw)?)
        })
        .await?
    }

    async fn region_status(
        &self,
        id: OfflineRegionId,
    ) -> Result<OfflineRegionStatus, OfflineStoreError> {
        let inner = Arc::clone(&self.inner);
        run_cpu_bound(move || {
            let conn = inner.lock();
            query_region_status(&conn, id)
        })
        .await?
    }

    async fn set_region_required_count(
        &self,
        id: OfflineRegionId,
        required_resource_count: u64,
    ) -> Result<(), OfflineStoreError> {
        let inner = Arc::clone(&self.inner);
        run_cpu_bound(move || {
            let conn = inner.lock();
            let changed = conn
                .execute(
                    "UPDATE regions SET required_count = ?2 WHERE id = ?1",
                    params![id.0, required_resource_count as i64],
                )
                .map_err(sql_err)?;
            if changed == 0 {
                return Err(OfflineStoreError::RegionNotFound(id));
            }
            Ok(())
        })
        .await?
    }

    async fn mark_region_complete(
        &self,
        id: OfflineRegionId,
        required_resource_count: u64,
    ) -> Result<(), OfflineStoreError> {
        let inner = Arc::clone(&self.inner);
        run_cpu_bound(move || {
            let conn = inner.lock();
            let changed = conn
                .execute(
                    "UPDATE regions SET complete = 1, required_count = ?2 WHERE id = ?1",
                    params![id.0, required_resource_count as i64],
                )
                .map_err(sql_err)?;
            if changed == 0 {
                return Err(OfflineStoreError::RegionNotFound(id));
            }
            Ok(())
        })
        .await?
    }

    async fn get_resource(&self, url: &str) -> Result<Option<Vec<u8>>, OfflineStoreError> {
        let url_owned = url.to_owned();
        let inner = Arc::clone(&self.inner);
        run_cpu_bound(move || {
            let conn = inner.lock();
            let row: Option<(Vec<u8>, i64)> = conn
                .query_row(
                    "SELECT data, compressed FROM resources WHERE url = ?1",
                    params![url_owned],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(sql_err)?;
            let Some((blob, compressed)) = row else {
                return Ok(None);
            };
            let stamp = inner.next_stamp();
            let _ = conn.execute(
                "UPDATE resources SET accessed_at = ?2 WHERE url = ?1",
                params![url_owned, stamp],
            );
            decode_resource_blob(blob, compressed != 0).map(Some)
        })
        .await?
    }

    async fn put_region_resource(
        &self,
        region_id: OfflineRegionId,
        url: &str,
        kind: ResourceKind,
        data: &[u8],
    ) -> Result<(), OfflineStoreError> {
        let url_owned = url.to_owned();
        let raw_len = data.len() as i64;
        let (encoded, compressed) = encode_resource_blob(data);
        let stored_len = encoded.len() as i64;
        let inner = Arc::clone(&self.inner);
        run_cpu_bound(move || {
            let mut conn = inner.lock();
            let tx = conn.transaction().map_err(sql_err)?;
            let exists: Option<i64> = tx
                .query_row("SELECT id FROM regions WHERE id = ?1", params![region_id.0], |r| {
                    r.get(0)
                })
                .optional()
                .map_err(sql_err)?;
            if exists.is_none() {
                return Err(OfflineStoreError::RegionNotFound(region_id));
            }
            if kind == ResourceKind::Tile && !is_url_pinned(&tx, &url_owned)? {
                if let Some(limit) = read_tile_limit(&tx)? {
                    if pinned_tile_count(&tx)? >= limit {
                        return Err(OfflineStoreError::TileLimitExceeded { limit });
                    }
                }
            }
            let stamp = inner.next_stamp();
            tx.execute(
                "INSERT INTO resources (url, kind, data, compressed, raw_size, stored_size, ambient, accessed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7)
                 ON CONFLICT(url) DO UPDATE SET
                    kind = excluded.kind,
                    data = excluded.data,
                    compressed = excluded.compressed,
                    raw_size = excluded.raw_size,
                    stored_size = excluded.stored_size,
                    accessed_at = excluded.accessed_at",
                params![
                    url_owned,
                    kind_to_i32(kind),
                    encoded,
                    i32::from(compressed),
                    raw_len,
                    stored_len,
                    stamp
                ],
            )
            .map_err(sql_err)?;
            tx.execute(
                "INSERT OR IGNORE INTO region_resources (region_id, url) VALUES (?1, ?2)",
                params![region_id.0, url_owned],
            )
            .map_err(sql_err)?;
            tx.commit().map_err(sql_err)?;
            Ok(())
        })
        .await?
    }

    async fn link_existing_resource(
        &self,
        region_id: OfflineRegionId,
        url: &str,
    ) -> Result<bool, OfflineStoreError> {
        let url_owned = url.to_owned();
        let inner = Arc::clone(&self.inner);
        run_cpu_bound(move || {
            let mut conn = inner.lock();
            let tx = conn.transaction().map_err(sql_err)?;
            let reg_exists: Option<i64> = tx
                .query_row(
                    "SELECT id FROM regions WHERE id = ?1",
                    params![region_id.0],
                    |r| r.get(0),
                )
                .optional()
                .map_err(sql_err)?;
            if reg_exists.is_none() {
                return Err(OfflineStoreError::RegionNotFound(region_id));
            }
            let res_kind: Option<i32> = tx
                .query_row(
                    "SELECT kind FROM resources WHERE url = ?1",
                    params![url_owned],
                    |r| r.get(0),
                )
                .optional()
                .map_err(sql_err)?;
            let Some(kind_code) = res_kind else {
                return Ok(false);
            };
            if kind_code == kind_to_i32(ResourceKind::Tile) && !is_url_pinned(&tx, &url_owned)? {
                if let Some(limit) = read_tile_limit(&tx)? {
                    if pinned_tile_count(&tx)? >= limit {
                        return Err(OfflineStoreError::TileLimitExceeded { limit });
                    }
                }
            }
            tx.execute(
                "INSERT OR IGNORE INTO region_resources (region_id, url) VALUES (?1, ?2)",
                params![region_id.0, url_owned],
            )
            .map_err(sql_err)?;
            tx.commit().map_err(sql_err)?;
            Ok(true)
        })
        .await?
    }

    async fn put_ambient_resource(
        &self,
        url: &str,
        kind: ResourceKind,
        data: &[u8],
    ) -> Result<(), OfflineStoreError> {
        let url_owned = url.to_owned();
        let raw_len = data.len() as i64;
        let (encoded, compressed) = encode_resource_blob(data);
        let stored_len = encoded.len() as i64;
        let inner = Arc::clone(&self.inner);
        run_cpu_bound(move || {
            let mut conn = inner.lock();
            let budget = read_max_ambient_bytes(&conn)?;
            if budget == 0 || (stored_len as u64) > budget {
                return Ok(());
            }
            let tx = conn.transaction().map_err(sql_err)?;
            let stamp = inner.next_stamp();
            tx.execute(
                "INSERT INTO resources (url, kind, data, compressed, raw_size, stored_size, ambient, accessed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7)
                 ON CONFLICT(url) DO UPDATE SET
                    data = excluded.data,
                    compressed = excluded.compressed,
                    raw_size = excluded.raw_size,
                    stored_size = excluded.stored_size,
                    accessed_at = excluded.accessed_at",
                params![
                    url_owned,
                    kind_to_i32(kind),
                    encoded,
                    i32::from(compressed),
                    raw_len,
                    stored_len,
                    stamp
                ],
            )
            .map_err(sql_err)?;
            enforce_ambient_budget(&tx)?;
            tx.commit().map_err(sql_err)?;
            Ok(())
        })
        .await?
    }

    async fn delete_region(&self, id: OfflineRegionId) -> Result<(), OfflineStoreError> {
        let inner = Arc::clone(&self.inner);
        run_cpu_bound(move || {
            let mut conn = inner.lock();
            let tx = conn.transaction().map_err(sql_err)?;
            tx.execute(
                "DELETE FROM region_resources WHERE region_id = ?1",
                params![id.0],
            )
            .map_err(sql_err)?;
            let deleted = tx
                .execute("DELETE FROM regions WHERE id = ?1", params![id.0])
                .map_err(sql_err)?;
            if deleted == 0 {
                return Err(OfflineStoreError::RegionNotFound(id));
            }
            gc_orphaned_non_ambient(&tx)?;
            enforce_ambient_budget(&tx)?;
            tx.commit().map_err(sql_err)?;
            Ok(())
        })
        .await?
    }

    async fn invalidate_region(&self, id: OfflineRegionId) -> Result<(), OfflineStoreError> {
        let inner = Arc::clone(&self.inner);
        run_cpu_bound(move || {
            let mut conn = inner.lock();
            let tx = conn.transaction().map_err(sql_err)?;
            let updated = tx
                .execute(
                    "UPDATE regions SET complete = 0 WHERE id = ?1",
                    params![id.0],
                )
                .map_err(sql_err)?;
            if updated == 0 {
                return Err(OfflineStoreError::RegionNotFound(id));
            }
            tx.execute(
                "DELETE FROM region_resources WHERE region_id = ?1",
                params![id.0],
            )
            .map_err(sql_err)?;
            gc_orphaned_non_ambient(&tx)?;
            enforce_ambient_budget(&tx)?;
            tx.commit().map_err(sql_err)?;
            Ok(())
        })
        .await?
    }

    async fn set_maximum_ambient_bytes(&self, max_bytes: u64) -> Result<(), OfflineStoreError> {
        let inner = Arc::clone(&self.inner);
        run_cpu_bound(move || {
            let mut conn = inner.lock();
            let tx = conn.transaction().map_err(sql_err)?;
            let clamped = i64::try_from(max_bytes).unwrap_or(i64::MAX);
            tx.execute(
                "INSERT INTO offline_config (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![CFG_MAX_AMBIENT_BYTES, clamped],
            )
            .map_err(sql_err)?;
            enforce_ambient_budget(&tx)?;
            tx.commit().map_err(sql_err)?;
            Ok(())
        })
        .await?
    }

    async fn set_tile_count_limit(&self, limit: Option<u64>) -> Result<(), OfflineStoreError> {
        let inner = Arc::clone(&self.inner);
        run_cpu_bound(move || {
            let conn = inner.lock();
            match limit {
                Some(max_tiles) => {
                    let clamped = i64::try_from(max_tiles).unwrap_or(i64::MAX);
                    conn.execute(
                        "INSERT INTO offline_config (key, value) VALUES (?1, ?2)
                         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                        params![CFG_TILE_COUNT_LIMIT, clamped],
                    )
                    .map_err(sql_err)?;
                }
                None => {
                    conn.execute(
                        "DELETE FROM offline_config WHERE key = ?1",
                        params![CFG_TILE_COUNT_LIMIT],
                    )
                    .map_err(sql_err)?;
                }
            }
            Ok(())
        })
        .await?
    }
}
