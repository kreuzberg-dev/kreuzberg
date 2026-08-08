//! Core cache implementation with GenericCache struct.
//!
//! # Lock Strategy
//!
//! This module uses `Arc<parking_lot::RwLock<T>>` for thread-safe state management.
//! Read-heavy operations (membership checks on every `get`/`is_processing` call) acquire
//! a shared read lock; mutations (insert/remove) acquire an exclusive write lock.
//!
//! `parking_lot::RwLock` is preferred over `std::sync::RwLock` because it is not
//! susceptible to lock poisoning, making the API infallible and avoiding
//! `XbergError::LockPoisoned` error paths for these fields.

use crate::error::{Result, XbergError};
use crate::telemetry::conventions;
use ahash::AHashSet;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use super::cleanup::smart_cleanup_cache;
use super::namespace::validate_namespace;
use super::version::versioned_cache_key;

/// Minimum seconds between automatic cleanup runs (5 minutes).
const CLEANUP_INTERVAL_SECS: u64 = 300;

/// Aggregate statistics for a xberg cache directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheStats {
    /// Total number of files currently in the cache directory.
    pub total_files: usize,
    /// Combined size of all cache files in megabytes.
    pub total_size_mb: f64,
    /// Free disk space available on the cache volume, in megabytes.
    pub available_space_mb: f64,
    /// Age of the oldest cache file in days (0.0 if the cache is empty).
    pub oldest_file_age_days: f64,
    /// Age of the most recently written cache file in days (0.0 if the cache is empty).
    pub newest_file_age_days: f64,
}

#[derive(Debug, Clone)]
pub(super) struct CacheEntry {
    pub(super) path: PathBuf,
    pub(super) size: u64,
    pub(super) modified: SystemTime,
}

pub(super) struct CacheScanResult {
    pub(super) stats: CacheStats,
    pub(super) entries: Vec<CacheEntry>,
}
/// Thread-safe, filesystem-backed cache with automatic LRU-style eviction.
///
/// `GenericCache` stores opaque blobs keyed by a content-hash string. It tracks
/// in-flight deletions to prevent read-during-delete races and periodically runs
/// a background cleanup that enforces the configured age and size limits.
#[cfg_attr(alef, alef(skip))]
pub struct GenericCache {
    cache_dir: PathBuf,
    #[cfg(test)]
    cache_type: String,
    max_age_days: f64,
    max_cache_size_mb: f64,
    min_free_space_mb: f64,
    #[cfg(test)]
    processing_locks: Arc<RwLock<AHashSet<String>>>,
    /// Tracks cache keys being deleted to prevent read-during-delete race conditions
    deleting_files: Arc<RwLock<AHashSet<PathBuf>>>,
}

impl GenericCache {
    pub(crate) fn new(
        cache_type: String,
        cache_dir: Option<String>,
        max_age_days: f64,
        max_cache_size_mb: f64,
        min_free_space_mb: f64,
    ) -> Result<Self> {
        let cache_dir_path = if let Some(dir) = cache_dir {
            PathBuf::from(dir).join(&cache_type)
        } else {
            crate::cache_dir::resolve_cache_dir(&cache_type)
        };

        fs::create_dir_all(&cache_dir_path)
            .map_err(|e| XbergError::cache(format!("Failed to create cache directory: {}", e)))?;

        Ok(Self {
            cache_dir: cache_dir_path,
            #[cfg(test)]
            cache_type,
            max_age_days,
            max_cache_size_mb,
            min_free_space_mb,
            #[cfg(test)]
            processing_locks: Arc::new(RwLock::new(AHashSet::new())),
            deleting_files: Arc::new(RwLock::new(AHashSet::new())),
        })
    }

    /// Acquire a shared read guard on `processing_locks`.
    ///
    /// `parking_lot::RwLock` is infallible (no poisoning), so this never returns an error.
    #[cfg(test)]
    fn read_processing_locks(&self) -> parking_lot::RwLockReadGuard<'_, AHashSet<String>> {
        self.processing_locks.read()
    }

    /// Acquire an exclusive write guard on `processing_locks`.
    #[cfg(test)]
    fn write_processing_locks(&self) -> parking_lot::RwLockWriteGuard<'_, AHashSet<String>> {
        self.processing_locks.write()
    }

    /// Acquire a shared read guard on `deleting_files`.
    fn read_deleting_files(&self) -> parking_lot::RwLockReadGuard<'_, AHashSet<PathBuf>> {
        self.deleting_files.read()
    }

    /// Acquire an exclusive write guard on `deleting_files`.
    #[cfg(test)]
    fn write_deleting_files(&self) -> parking_lot::RwLockWriteGuard<'_, AHashSet<PathBuf>> {
        self.deleting_files.write()
    }

    /// Resolve the directory for a cache key, optionally within a namespace subdirectory.
    fn resolve_dir(&self, namespace: Option<&str>) -> PathBuf {
        match namespace {
            Some(ns) => self.cache_dir.join(ns),
            None => self.cache_dir.clone(),
        }
    }

    fn get_cache_path(&self, cache_key: &str, namespace: Option<&str>) -> PathBuf {
        self.resolve_dir(namespace).join(format!("{}.msgpack", cache_key))
    }

    fn get_metadata_path(&self, cache_key: &str, namespace: Option<&str>) -> PathBuf {
        self.resolve_dir(namespace).join(format!("{}.meta", cache_key))
    }

    fn is_valid(&self, cache_path: &Path, source_file: Option<&str>, ttl_override_secs: Option<u64>) -> bool {
        if !cache_path.exists() {
            return false;
        }

        if let Ok(metadata) = fs::metadata(cache_path)
            && let Ok(modified) = metadata.modified()
            && let Ok(elapsed) = SystemTime::now().duration_since(modified)
        {
            let max_age_secs = if let Some(ttl) = ttl_override_secs {
                ttl as f64
            } else if let Some(meta_ttl) = self.read_meta_ttl(cache_path) {
                if meta_ttl > 0 {
                    meta_ttl as f64
                } else {
                    self.max_age_days * 86400.0
                }
            } else {
                self.max_age_days * 86400.0
            };

            if elapsed.as_secs_f64() > max_age_secs {
                return false;
            }
        }

        if let Some(source_path) = source_file {
            let Some(file_stem) = cache_path.file_stem().and_then(|s| s.to_str()) else {
                return false;
            };
            let namespace = self.infer_namespace(cache_path);
            let meta_path = self.get_metadata_path(file_stem, namespace.as_deref());

            if meta_path.exists() {
                if let Ok(cached_meta_bytes) = fs::read(&meta_path)
                    && cached_meta_bytes.len() >= 16
                {
                    let cached_size = u64::from_le_bytes(cached_meta_bytes[0..8].try_into().unwrap());
                    let cached_mtime = u64::from_le_bytes(cached_meta_bytes[8..16].try_into().unwrap());

                    if let Ok(source_metadata) = fs::metadata(source_path) {
                        let current_size = source_metadata.len();
                        let Some(current_mtime) = source_metadata
                            .modified()
                            .ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_secs())
                        else {
                            return false;
                        };

                        return cached_size == current_size && cached_mtime == current_mtime;
                    }
                }
                return false;
            }
        }

        true
    }

    /// Read TTL from .meta file (bytes 16-23). Returns None if not present.
    fn read_meta_ttl(&self, cache_path: &Path) -> Option<u64> {
        let file_stem = cache_path.file_stem()?.to_str()?;
        let namespace = self.infer_namespace(cache_path);
        let meta_path = self.get_metadata_path(file_stem, namespace.as_deref());
        let bytes = fs::read(&meta_path).ok()?;
        if bytes.len() >= 24 {
            Some(u64::from_le_bytes(bytes[16..24].try_into().unwrap()))
        } else {
            None
        }
    }

    /// Infer namespace from a cache path by checking if it's in a subdirectory.
    fn infer_namespace(&self, cache_path: &Path) -> Option<String> {
        let parent = cache_path.parent()?;
        if parent == self.cache_dir {
            None
        } else {
            parent.file_name()?.to_str().map(|s| s.to_string())
        }
    }

    fn save_metadata(
        &self,
        cache_key: &str,
        source_file: Option<&str>,
        namespace: Option<&str>,
        ttl_secs: Option<u64>,
    ) {
        let meta_path = self.get_metadata_path(cache_key, namespace);

        let mut bytes = Vec::with_capacity(24);

        if let Some(source_path) = source_file
            && let Ok(metadata) = fs::metadata(source_path)
        {
            let size = metadata.len();
            let mtime = metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);

            bytes.extend_from_slice(&size.to_le_bytes());
            bytes.extend_from_slice(&mtime.to_le_bytes());
        } else {
            bytes.extend_from_slice(&0u64.to_le_bytes());
            bytes.extend_from_slice(&0u64.to_le_bytes());
        }

        bytes.extend_from_slice(&ttl_secs.unwrap_or(0).to_le_bytes());

        if let Err(e) = fs::write(&meta_path, bytes) {
            // Not fatal — the blob is already on disk and `is_valid` degrades to
            // the age check — but a persistent failure here silently disables
            // source-file and TTL invalidation, so it must not be swallowed.
            tracing::warn!(
                { conventions::OPERATION } = conventions::operations::CACHE_WRITE,
                { conventions::CACHE_KEY } = cache_key,
                error = %e,
                "Failed to write cache metadata sidecar; source-file and TTL invalidation are disabled for this entry"
            );
        }
    }

    /// Validate a caller-supplied namespace, logging and wrapping the rejection.
    ///
    /// A namespace becomes a directory component under the cache root, so an
    /// unvalidated one is a path-traversal primitive. See [`validate_namespace`].
    fn check_namespace(namespace: Option<&str>, operation: &'static str) -> Result<()> {
        let Some(namespace) = namespace else {
            return Ok(());
        };

        validate_namespace(namespace).map_err(|e| {
            tracing::warn!(
                { conventions::OPERATION } = operation,
                error = %e,
                "Rejected cache namespace"
            );
            e
        })
    }

    /// Emit cache-lookup telemetry for a resolved key.
    fn record_lookup(versioned_key: &str, hit: bool) {
        #[cfg(feature = "otel")]
        {
            tracing::Span::current().record("cache.hit", hit);

            let metrics = crate::telemetry::metrics::get_metrics();
            if hit {
                metrics.cache_hits.add(1, &[]);
            } else {
                metrics.cache_misses.add(1, &[]);
            }
        }

        tracing::debug!(
            { conventions::OPERATION } = conventions::operations::CACHE_LOOKUP,
            { conventions::CACHE_KEY } = versioned_key,
            { conventions::CACHE_HIT } = hit,
            "Cache lookup"
        );
    }

    #[cfg_attr(feature = "otel", tracing::instrument(
        skip(self),
        fields(
            cache.hit = tracing::field::Empty,
            cache.key = %cache_key,
        )
    ))]
    pub(crate) fn get(
        &self,
        cache_key: &str,
        source_file: Option<&str>,
        namespace: Option<&str>,
        ttl_override_secs: Option<u64>,
    ) -> Result<Option<Vec<u8>>> {
        Self::check_namespace(namespace, conventions::operations::CACHE_LOOKUP)?;

        let versioned_key = versioned_cache_key(cache_key);
        let cache_path = self.get_cache_path(&versioned_key, namespace);

        {
            let deleting = self.read_deleting_files();
            if deleting.contains(&cache_path) {
                Self::record_lookup(&versioned_key, false);
                return Ok(None);
            }
        }

        if !self.is_valid(&cache_path, source_file, ttl_override_secs) {
            Self::record_lookup(&versioned_key, false);
            return Ok(None);
        }

        match fs::read(&cache_path) {
            Ok(content) => {
                Self::record_lookup(&versioned_key, true);
                Ok(Some(content))
            }
            Err(e) => {
                tracing::warn!(
                    { conventions::OPERATION } = conventions::operations::CACHE_LOOKUP,
                    { conventions::CACHE_KEY } = versioned_key.as_str(),
                    error = %e,
                    "Failed to read a cache entry that passed validation; discarding it and treating the lookup as a miss"
                );
                if let Err(e) = fs::remove_file(&cache_path) {
                    tracing::debug!("Failed to remove corrupted cache file: {}", e);
                }
                let meta_path = self.get_metadata_path(&versioned_key, namespace);
                if let Err(e) = fs::remove_file(meta_path) {
                    tracing::debug!("Failed to remove corrupted metadata file: {}", e);
                }
                Self::record_lookup(&versioned_key, false);
                Ok(None)
            }
        }
    }

    /// Backward-compatible get without namespace/TTL.
    #[cfg(test)]
    pub(crate) fn get_default(&self, cache_key: &str, source_file: Option<&str>) -> Result<Option<Vec<u8>>> {
        self.get(cache_key, source_file, None, None)
    }

    #[cfg_attr(feature = "otel", tracing::instrument(
        skip(self, data),
        fields(
            cache.key = %cache_key,
            cache.size_bytes = data.len(),
        )
    ))]
    pub(crate) fn set(
        &self,
        cache_key: &str,
        data: Vec<u8>,
        source_file: Option<&str>,
        namespace: Option<&str>,
        ttl_secs: Option<u64>,
    ) -> Result<()> {
        // Validate BEFORE `create_dir_all` — an unvalidated namespace is what
        // turns the directory creation below into a path-traversal primitive.
        Self::check_namespace(namespace, conventions::operations::CACHE_WRITE)?;

        let versioned_key = versioned_cache_key(cache_key);
        let dir = self.resolve_dir(namespace);
        fs::create_dir_all(&dir).map_err(|e| {
            tracing::warn!(
                { conventions::OPERATION } = conventions::operations::CACHE_WRITE,
                { conventions::CACHE_KEY } = versioned_key.as_str(),
                error = %e,
                "Failed to create the cache namespace directory; the result will not be cached"
            );
            XbergError::cache(format!("Failed to create cache namespace dir: {}", e))
        })?;

        let cache_path = self.get_cache_path(&versioned_key, namespace);

        fs::write(&cache_path, &data).map_err(|e| {
            tracing::warn!(
                { conventions::OPERATION } = conventions::operations::CACHE_WRITE,
                { conventions::CACHE_KEY } = versioned_key.as_str(),
                error = %e,
                "Failed to write the cache entry; the result will not be cached"
            );
            XbergError::cache(format!("Failed to write cache file: {}", e))
        })?;

        self.save_metadata(&versioned_key, source_file, namespace, ttl_secs);

        tracing::debug!(
            { conventions::OPERATION } = conventions::operations::CACHE_WRITE,
            { conventions::CACHE_KEY } = versioned_key.as_str(),
            size_bytes = data.len(),
            "Cache write"
        );

        if self.should_run_cleanup() {
            if let Some(cache_path_str) = self.cache_dir.to_str()
                && let Err(e) = smart_cleanup_cache(
                    cache_path_str,
                    self.max_age_days,
                    self.max_cache_size_mb,
                    self.min_free_space_mb,
                )
            {
                // Swallowing this hides an unbounded cache from the operator.
                tracing::warn!(
                    { conventions::OPERATION } = conventions::operations::CACHE_WRITE,
                    error = %e,
                    "Cache cleanup failed; the cache may exceed its configured size and age limits"
                );
            }
            self.touch_cleanup_marker();
        }

        Ok(())
    }

    /// Backward-compatible set without namespace/TTL.
    #[cfg(test)]
    pub(crate) fn set_default(&self, cache_key: &str, data: Vec<u8>, source_file: Option<&str>) -> Result<()> {
        self.set(cache_key, data, source_file, None, None)
    }

    /// Check if cleanup should run based on filesystem marker timestamp.
    ///
    /// Multi-worker safe: uses filesystem mtime instead of in-memory counter.
    fn should_run_cleanup(&self) -> bool {
        let marker = self.cache_dir.join(".last_cleanup");
        match fs::metadata(&marker) {
            Ok(meta) => {
                if let Ok(modified) = meta.modified() {
                    let age = SystemTime::now().duration_since(modified).unwrap_or_default();
                    age.as_secs() > CLEANUP_INTERVAL_SECS
                } else {
                    true
                }
            }
            Err(_) => true,
        }
    }

    /// Touch the cleanup marker file to record last cleanup time.
    fn touch_cleanup_marker(&self) {
        let marker = self.cache_dir.join(".last_cleanup");
        if let Err(e) = fs::write(&marker, []) {
            // Without the marker every `set` re-runs the cleanup scan.
            tracing::debug!("Failed to touch the cache cleanup marker: {}", e);
        }
    }

    #[cfg(test)]
    pub(crate) fn is_processing(&self, cache_key: &str) -> Result<bool> {
        Ok(self.read_processing_locks().contains(cache_key))
    }

    #[cfg(test)]
    pub(crate) fn mark_processing(&self, cache_key: String) -> Result<()> {
        self.write_processing_locks().insert(cache_key);
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn mark_complete(&self, cache_key: &str) -> Result<()> {
        self.write_processing_locks().remove(cache_key);
        Ok(())
    }

    #[cfg(test)]
    fn mark_for_deletion(&self, path: &Path) -> Result<()> {
        self.write_deleting_files().insert(path.to_path_buf());
        Ok(())
    }

    #[cfg(test)]
    fn unmark_deletion(&self, path: &Path) -> Result<()> {
        self.write_deleting_files().remove(&path.to_path_buf());
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn clear(&self) -> Result<(usize, f64)> {
        let dir_path = &self.cache_dir;

        if !dir_path.exists() {
            return Ok((0, 0.0));
        }

        let mut removed_count = 0;
        let mut removed_size = 0.0;

        let read_dir =
            fs::read_dir(dir_path).map_err(|e| XbergError::cache(format!("Failed to read cache directory: {}", e)))?;

        for entry in read_dir {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    tracing::debug!("Error reading entry: {}", e);
                    continue;
                }
            };

            let path = entry.path();

            if path.file_name().and_then(|n| n.to_str()) == Some(".last_cleanup") {
                continue;
            }

            let metadata = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };

            if metadata.is_dir() {
                let (ns_removed, ns_freed) = self.delete_namespace_inner(&path)?;
                removed_count += ns_removed;
                removed_size += ns_freed;
                continue;
            }

            if !metadata.is_file() {
                continue;
            }

            let ext = path.extension().and_then(|s| s.to_str());
            if ext != Some("msgpack") && ext != Some("meta") {
                continue;
            }

            let size_mb = metadata.len() as f64 / (1024.0 * 1024.0);

            if let Err(e) = self.mark_for_deletion(&path) {
                tracing::debug!("Failed to mark file for deletion: {} (continuing anyway)", e);
            }

            match fs::remove_file(&path) {
                Ok(_) => {
                    removed_count += 1;
                    removed_size += size_mb;
                    if let Err(e) = self.unmark_deletion(&path) {
                        tracing::debug!("Failed to unmark deleted file: {} (non-critical)", e);
                    }
                }
                Err(e) => {
                    tracing::debug!("Failed to remove {:?}: {}", path, e);
                    if let Err(e) = self.unmark_deletion(&path) {
                        tracing::debug!("Failed to unmark file after deletion error: {} (non-critical)", e);
                    }
                }
            }
        }

        Ok((removed_count, removed_size))
    }

    /// Remove a directory and count its contents (used by `clear_all` to handle namespace dirs).
    #[cfg(test)]
    fn delete_namespace_inner(&self, dir: &Path) -> Result<(usize, f64)> {
        if !dir.exists() {
            return Ok((0, 0.0));
        }

        let mut removed_count = 0;
        let mut removed_size = 0.0;

        if let Ok(read_dir) = fs::read_dir(dir) {
            for entry in read_dir.flatten() {
                if let Ok(meta) = entry.metadata()
                    && meta.is_file()
                {
                    removed_size += meta.len() as f64 / (1024.0 * 1024.0);
                    removed_count += 1;
                }
            }
        }

        fs::remove_dir_all(dir)
            .map_err(|e| XbergError::cache(format!("Failed to remove directory {}: {}", dir.display(), e)))?;

        Ok((removed_count, removed_size))
    }

    #[cfg(test)]
    pub(crate) fn get_stats(&self) -> Result<CacheStats> {
        self.get_stats_filtered(None)
    }

    /// Get cache stats, optionally filtered to a specific namespace.
    #[cfg(test)]
    pub(crate) fn get_stats_filtered(&self, namespace: Option<&str>) -> Result<CacheStats> {
        let dir = self.resolve_dir(namespace);
        let dir_str = dir
            .to_str()
            .ok_or_else(|| XbergError::validation("Cache directory path contains invalid UTF-8".to_string()))?;
        super::cleanup::get_cache_metadata(dir_str)
    }

    #[cfg(test)]
    pub(crate) fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    #[cfg(test)]
    pub(crate) fn cache_type(&self) -> &str {
        &self.cache_type
    }
}
