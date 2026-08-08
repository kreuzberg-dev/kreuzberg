//! Generic cache implementation with lock poisoning recovery.
//!
//! This module provides a thread-safe caching system with automatic cleanup,
//! processing locks, and validation capabilities.

mod cleanup;
mod core;
mod namespace;
mod utilities;
pub(crate) mod version;

pub use cleanup::{clear_cache_directory, get_cache_metadata};
pub use core::{CacheStats, GenericCache};
#[cfg(feature = "ocr")]
pub(crate) use utilities::blake3_hash_bytes;
pub(crate) use utilities::blake3_hash_file;
#[cfg(test)]
pub(crate) use utilities::{fast_hash, generate_cache_key, validate_cache_key};

#[cfg(test)]
mod tests {
    use super::*;
    use cleanup::cleanup_cache;
    use std::fs::File;
    use tempfile::tempdir;

    #[test]
    fn test_generate_cache_key_empty() {
        let result = generate_cache_key(&[]);
        assert_eq!(result, "empty");
    }

    #[test]
    fn test_generate_cache_key_consistent() {
        let parts = vec![
            ("key1".to_string(), "value1".to_string()),
            ("key2".to_string(), "value2".to_string()),
        ];
        let key1 = generate_cache_key(&parts);
        let key2 = generate_cache_key(&parts);
        assert_eq!(key1, key2);
        assert_eq!(key1.len(), 32);
    }

    #[test]
    fn test_validate_cache_key() {
        assert!(validate_cache_key("0123456789abcdef0123456789abcdef"));
        assert!(!validate_cache_key("invalid_key"));
        assert!(!validate_cache_key("0123456789abcdef"));
        assert!(!validate_cache_key("0123456789abcdef0123456789abcdef0"));
    }

    #[test]
    fn test_fast_hash() {
        let data1 = b"test data";
        let data2 = b"test data";
        let data3 = b"different data";

        assert_eq!(fast_hash(data1), fast_hash(data2));
        assert_ne!(fast_hash(data1), fast_hash(data3));
    }

    #[test]
    fn test_cache_metadata() {
        let temp_dir = tempdir().unwrap();
        let cache_dir = temp_dir.path().to_str().unwrap();

        let file1 = temp_dir.path().join("test1.msgpack");
        let file2 = temp_dir.path().join("test2.msgpack");
        File::create(&file1).unwrap();
        File::create(&file2).unwrap();

        let stats = get_cache_metadata(cache_dir).unwrap();
        assert_eq!(stats.total_files, 2);
        assert!(stats.available_space_mb > 0.0);
    }

    #[test]
    fn test_cleanup_cache() {
        use std::io::Write;

        let temp_dir = tempdir().unwrap();
        let cache_dir = temp_dir.path().to_str().unwrap();

        let file1 = temp_dir.path().join("old.msgpack");
        let mut f = File::create(&file1).unwrap();
        f.write_all(b"test data for cleanup").unwrap();
        drop(f);

        let (removed_count, _) = cleanup_cache(cache_dir, 1000.0, 0.000001, 0.8).unwrap();
        assert_eq!(removed_count, 1);
        assert!(!file1.exists());
    }

    #[test]
    fn test_generic_cache_new() {
        let temp_dir = tempdir().unwrap();
        let cache = GenericCache::new(
            "test".to_string(),
            Some(temp_dir.path().to_str().unwrap().to_string()),
            30.0,
            500.0,
            1000.0,
        )
        .unwrap();

        assert_eq!(cache.cache_type(), "test");
        assert!(cache.cache_dir().exists());
    }

    #[test]
    fn test_generic_cache_get_set() {
        let temp_dir = tempdir().unwrap();
        let cache = GenericCache::new(
            "test".to_string(),
            Some(temp_dir.path().to_str().unwrap().to_string()),
            30.0,
            500.0,
            1000.0,
        )
        .unwrap();

        let cache_key = "test_key";
        let data = b"test data".to_vec();

        cache.set_default(cache_key, data.clone(), None).unwrap();

        let result = cache.get_default(cache_key, None).unwrap();
        assert_eq!(result, Some(data));
    }

    #[test]
    fn test_generic_cache_get_miss() {
        let temp_dir = tempdir().unwrap();
        let cache = GenericCache::new(
            "test".to_string(),
            Some(temp_dir.path().to_str().unwrap().to_string()),
            30.0,
            500.0,
            1000.0,
        )
        .unwrap();

        let result = cache.get_default("nonexistent", None).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_generic_cache_source_file_invalidation() {
        use std::io::Write;
        use std::thread::sleep;
        use std::time::Duration;

        let temp_dir = tempdir().unwrap();
        let cache = GenericCache::new(
            "test".to_string(),
            Some(temp_dir.path().to_str().unwrap().to_string()),
            30.0,
            500.0,
            1000.0,
        )
        .unwrap();

        let source_file = temp_dir.path().join("source.txt");
        let mut f = File::create(&source_file).unwrap();
        f.write_all(b"original content").unwrap();
        drop(f);

        let cache_key = "test_key";
        let data = b"cached data".to_vec();

        cache
            .set_default(cache_key, data.clone(), Some(source_file.to_str().unwrap()))
            .unwrap();

        let result = cache
            .get_default(cache_key, Some(source_file.to_str().unwrap()))
            .unwrap();
        assert_eq!(result, Some(data.clone()));

        sleep(Duration::from_millis(10));
        use std::fs;
        let mut f = fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&source_file)
            .unwrap();
        f.write_all(b"modified content with different size").unwrap();
        drop(f);

        let result = cache
            .get_default(cache_key, Some(source_file.to_str().unwrap()))
            .unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_generic_cache_processing_locks() {
        let temp_dir = tempdir().unwrap();
        let cache = GenericCache::new(
            "test".to_string(),
            Some(temp_dir.path().to_str().unwrap().to_string()),
            30.0,
            500.0,
            1000.0,
        )
        .unwrap();

        let cache_key = "test_key";

        assert!(!cache.is_processing(cache_key).unwrap());

        cache.mark_processing(cache_key.to_string()).unwrap();
        assert!(cache.is_processing(cache_key).unwrap());

        cache.mark_complete(cache_key).unwrap();
        assert!(!cache.is_processing(cache_key).unwrap());
    }

    #[test]
    fn test_generic_cache_clear() {
        let temp_dir = tempdir().unwrap();
        let cache = GenericCache::new(
            "test".to_string(),
            Some(temp_dir.path().to_str().unwrap().to_string()),
            30.0,
            500.0,
            1000.0,
        )
        .unwrap();

        cache.set_default("key1", b"data1".to_vec(), None).unwrap();
        cache.set_default("key2", b"data2".to_vec(), None).unwrap();

        let (removed, _freed) = cache.clear().unwrap();
        assert!(removed >= 2, "Should remove at least 2 cache entries (got {})", removed);

        assert_eq!(cache.get_default("key1", None).unwrap(), None);
        assert_eq!(cache.get_default("key2", None).unwrap(), None);
    }

    #[test]
    fn test_generic_cache_stats() {
        let temp_dir = tempdir().unwrap();
        let cache = GenericCache::new(
            "test".to_string(),
            Some(temp_dir.path().to_str().unwrap().to_string()),
            30.0,
            500.0,
            1000.0,
        )
        .unwrap();

        cache.set_default("key1", b"test data 1".to_vec(), None).unwrap();
        cache.set_default("key2", b"test data 2".to_vec(), None).unwrap();

        let stats = cache.get_stats().unwrap();
        assert_eq!(stats.total_files, 2);
        assert!(stats.total_size_mb > 0.0);
        assert!(stats.available_space_mb > 0.0);
    }

    #[test]
    fn test_generic_cache_expired_entry() {
        use std::io::Write;

        let temp_dir = tempdir().unwrap();
        let cache = GenericCache::new(
            "test".to_string(),
            Some(temp_dir.path().to_str().unwrap().to_string()),
            0.000001,
            500.0,
            1000.0,
        )
        .unwrap();

        let cache_key = "test_key";

        let cache_path = cache.cache_dir().join(format!("{}.msgpack", cache_key));
        let mut f = File::create(&cache_path).unwrap();
        f.write_all(b"test data").unwrap();
        drop(f);

        let old_time = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        filetime::set_file_mtime(&cache_path, filetime::FileTime::from_system_time(old_time)).unwrap();

        let result = cache.get_default(cache_key, None).unwrap();
        assert_eq!(result, None);
    }

    /// Build a cache rooted at `dir` with limits that never trigger cleanup.
    fn cache_at(dir: &std::path::Path) -> GenericCache {
        GenericCache::new(
            "test".to_string(),
            Some(dir.to_str().unwrap().to_string()),
            30.0,
            500.0,
            1000.0,
        )
        .unwrap()
    }

    // ---------------------------------------------------------------------
    // #199 — an unvalidated `cache_namespace` reaches `create_dir_all`.
    // ---------------------------------------------------------------------

    #[test]
    fn set_should_reject_a_traversing_namespace_and_create_no_directory_outside_the_cache_root() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path().join("root");
        let cache = cache_at(&root);

        let escaped = temp_dir.path().join("escaped");
        assert!(!escaped.exists(), "precondition: the escape target must not exist");

        let error = cache
            .set("key", b"data".to_vec(), None, Some("../../escaped"), None)
            .expect_err("a traversing namespace must be rejected");

        assert!(
            error.to_string().contains("Cache namespace"),
            "expected a namespace validation error, got: {error}"
        );
        assert!(
            !escaped.exists(),
            "namespace traversal created {} outside the cache root",
            escaped.display()
        );
    }

    #[test]
    fn set_should_reject_an_absolute_namespace() {
        let temp_dir = tempdir().unwrap();
        let cache = cache_at(temp_dir.path());

        let absolute = temp_dir.path().join("absolute-target");
        let absolute_namespace = absolute.to_str().unwrap();

        assert!(
            cache
                .set("key", b"data".to_vec(), None, Some(absolute_namespace), None)
                .is_err(),
            "an absolute namespace must be rejected"
        );
        assert!(!absolute.exists(), "an absolute namespace escaped the cache root");
    }

    #[test]
    fn set_should_reject_the_parent_directory_namespace() {
        let temp_dir = tempdir().unwrap();
        let cache = cache_at(temp_dir.path());

        assert!(cache.set("key", b"data".to_vec(), None, Some(".."), None).is_err());
        assert!(cache.set("key", b"data".to_vec(), None, Some("."), None).is_err());
    }

    #[test]
    fn get_should_reject_a_traversing_namespace() {
        let temp_dir = tempdir().unwrap();
        let cache = cache_at(temp_dir.path());

        let error = cache
            .get("key", None, Some("../../escaped"), None)
            .expect_err("a traversing namespace must be rejected on read too");
        assert!(
            error.to_string().contains("Cache namespace"),
            "expected a namespace validation error, got: {error}"
        );
    }

    #[test]
    fn set_and_get_should_still_accept_a_valid_tenant_namespace() {
        let temp_dir = tempdir().unwrap();
        let cache = cache_at(temp_dir.path());

        cache
            .set("key", b"tenant data".to_vec(), None, Some("tenant-123"), None)
            .expect("a plain tenant namespace must be accepted");

        assert_eq!(
            cache.get("key", None, Some("tenant-123"), None).unwrap(),
            Some(b"tenant data".to_vec())
        );
        assert!(
            cache.cache_dir().join("tenant-123").is_dir(),
            "the namespace directory must live directly under the cache root"
        );
    }

    #[test]
    fn namespaces_should_isolate_entries_from_each_other() {
        let temp_dir = tempdir().unwrap();
        let cache = cache_at(temp_dir.path());

        cache.set("key", b"a".to_vec(), None, Some("tenant-a"), None).unwrap();
        cache.set("key", b"b".to_vec(), None, Some("tenant-b"), None).unwrap();

        assert_eq!(
            cache.get("key", None, Some("tenant-a"), None).unwrap(),
            Some(b"a".to_vec())
        );
        assert_eq!(
            cache.get("key", None, Some("tenant-b"), None).unwrap(),
            Some(b"b".to_vec())
        );
        assert_eq!(cache.get("key", None, None, None).unwrap(), None);
    }

    // ---------------------------------------------------------------------
    // #206 — the extraction cache key carries no version fingerprint.
    // ---------------------------------------------------------------------

    #[test]
    fn on_disk_entry_should_be_prefixed_with_the_build_version_tag() {
        let temp_dir = tempdir().unwrap();
        let cache = cache_at(temp_dir.path());

        cache.set("abc123", b"payload".to_vec(), None, None, None).unwrap();

        let tag = version::cache_version_tag();
        let expected_blob = cache.cache_dir().join(format!("{tag}-abc123.msgpack"));
        let expected_meta = cache.cache_dir().join(format!("{tag}-abc123.meta"));

        assert!(
            expected_blob.is_file(),
            "expected the versioned blob at {}",
            expected_blob.display()
        );
        assert!(
            expected_meta.is_file(),
            "expected the versioned metadata sidecar at {}",
            expected_meta.display()
        );
        assert!(
            !cache.cache_dir().join("abc123.msgpack").exists(),
            "the unversioned key must no longer be written"
        );
    }

    #[test]
    fn entry_written_by_a_different_build_version_should_not_be_served() {
        use std::io::Write;

        let temp_dir = tempdir().unwrap();
        let cache = cache_at(temp_dir.path());

        // Simulate an entry left behind by an older build: same logical key,
        // different version tag.
        let stale = cache.cache_dir().join("00000000-abc123.msgpack");
        let mut file = File::create(&stale).unwrap();
        file.write_all(b"result from an older build").unwrap();
        drop(file);

        assert_eq!(
            cache.get("abc123", None, None, None).unwrap(),
            None,
            "an entry written under a different version tag must not be served"
        );
        assert!(stale.exists(), "the stale entry is left for the cleanup pass, not read");
    }

    #[test]
    fn round_trip_should_still_work_within_a_single_build() {
        let temp_dir = tempdir().unwrap();
        let cache = cache_at(temp_dir.path());

        cache.set("stable", b"payload".to_vec(), None, None, None).unwrap();
        assert_eq!(
            cache.get("stable", None, None, None).unwrap(),
            Some(b"payload".to_vec()),
            "versioning must not break the hit path within one build"
        );
        assert_eq!(
            cache.get("stable", None, None, None).unwrap(),
            Some(b"payload".to_vec()),
            "repeated reads must be stable"
        );
    }

    #[test]
    fn versioned_metadata_sidecar_should_still_invalidate_on_source_change() {
        use std::io::Write;
        use std::thread::sleep;
        use std::time::Duration;

        let temp_dir = tempdir().unwrap();
        let cache = cache_at(temp_dir.path());

        let source = temp_dir.path().join("source.txt");
        let mut file = File::create(&source).unwrap();
        file.write_all(b"original").unwrap();
        drop(file);
        let source_str = source.to_str().unwrap();

        cache
            .set("key", b"cached".to_vec(), Some(source_str), Some("tenant-a"), None)
            .unwrap();
        assert_eq!(
            cache.get("key", Some(source_str), Some("tenant-a"), None).unwrap(),
            Some(b"cached".to_vec()),
            "the sidecar must be found under the versioned key inside the namespace"
        );

        sleep(Duration::from_millis(10));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&source)
            .unwrap();
        file.write_all(b"modified content with a different size").unwrap();
        drop(file);

        assert_eq!(
            cache.get("key", Some(source_str), Some("tenant-a"), None).unwrap(),
            None,
            "a changed source file must still invalidate the versioned entry"
        );
    }

    // ---------------------------------------------------------------------
    // #272 — cache read and write failures are silently swallowed, and the
    // four `telemetry::conventions` cache constants are never emitted.
    // ---------------------------------------------------------------------

    /// Capture `tracing` output emitted on this thread while `body` runs.
    fn capture_logs<T>(body: impl FnOnce() -> T) -> (T, String) {
        use std::sync::{Arc, Mutex};

        #[derive(Clone)]
        struct Capture(Arc<Mutex<Vec<u8>>>);

        impl std::io::Write for Capture {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                // `write_all`, not `write`: a partial write would silently truncate the
                // captured log and turn this into an assertion failure with no explanation.
                self.0.lock().expect("log buffer poisoned").write_all(buf)?;
                Ok(buf.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let buffer = Arc::new(Mutex::new(Vec::new()));
        let capture = Capture(Arc::clone(&buffer));
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(move || capture.clone())
            .finish();

        // `with_default` installs a *thread-local* subscriber, but `tracing`'s callsite
        // interest cache is process-global. When these tests run in parallel with others
        // that emit at a higher level, a callsite can retain a cached "not interested"
        // verdict from before this subscriber existed, and the `debug!` events this test
        // asserts on are then never emitted at all — which showed up as a rare, otherwise
        // inexplicable failure under full-suite contention (#301). Forcing a rebuild makes
        // the capture depend only on the subscriber we just installed.
        tracing::callsite::rebuild_interest_cache();
        let value = tracing::subscriber::with_default(subscriber, body);
        let logs =
            String::from_utf8(buffer.lock().expect("log buffer poisoned").clone()).expect("log output must be UTF-8");
        (value, logs)
    }

    #[test]
    fn cache_lookup_should_emit_the_hit_and_key_conventions() {
        let temp_dir = tempdir().unwrap();
        let cache = cache_at(temp_dir.path());

        let (_, logs) = capture_logs(|| {
            assert_eq!(cache.get("telemetry", None, None, None).unwrap(), None);
            cache.set("telemetry", b"payload".to_vec(), None, None, None).unwrap();
            assert_eq!(
                cache.get("telemetry", None, None, None).unwrap(),
                Some(b"payload".to_vec())
            );
        });

        assert!(
            logs.contains(crate::telemetry::conventions::CACHE_KEY),
            "cache lookups must emit {}; logs were:\n{logs}",
            crate::telemetry::conventions::CACHE_KEY
        );
        assert!(
            logs.contains(&format!("{}=false", crate::telemetry::conventions::CACHE_HIT)),
            "a miss must emit {}=false; logs were:\n{logs}",
            crate::telemetry::conventions::CACHE_HIT
        );
        assert!(
            logs.contains(&format!("{}=true", crate::telemetry::conventions::CACHE_HIT)),
            "a hit must emit {}=true; logs were:\n{logs}",
            crate::telemetry::conventions::CACHE_HIT
        );
        assert!(
            logs.contains(crate::telemetry::conventions::OPERATION),
            "cache events must carry {}; logs were:\n{logs}",
            crate::telemetry::conventions::OPERATION
        );
        assert!(
            logs.contains(crate::telemetry::conventions::operations::CACHE_LOOKUP),
            "cache lookups must be labelled {}; logs were:\n{logs}",
            crate::telemetry::conventions::operations::CACHE_LOOKUP
        );
        assert!(
            logs.contains(crate::telemetry::conventions::operations::CACHE_WRITE),
            "cache writes must be labelled {}; logs were:\n{logs}",
            crate::telemetry::conventions::operations::CACHE_WRITE
        );
    }

    #[test]
    fn a_rejected_namespace_should_be_logged_not_swallowed() {
        let temp_dir = tempdir().unwrap();
        let cache = cache_at(temp_dir.path());

        let (_, logs) = capture_logs(|| {
            assert!(cache.get("key", None, Some("../escape"), None).is_err());
            assert!(cache.set("key", b"x".to_vec(), None, Some("../escape"), None).is_err());
        });

        assert!(
            logs.contains("Rejected cache namespace"),
            "a rejected namespace must be logged; logs were:\n{logs}"
        );
    }

    #[test]
    fn a_failed_cache_write_should_be_logged_and_returned_as_an_error() {
        let temp_dir = tempdir().unwrap();
        let cache = cache_at(temp_dir.path());

        // Occupy the namespace directory path with a regular file so
        // `create_dir_all` cannot succeed.
        let blocker = cache.cache_dir().join("blocked");
        std::fs::write(&blocker, b"not a directory").unwrap();

        let (error, logs) = capture_logs(|| {
            cache
                .set("key", b"payload".to_vec(), None, Some("blocked"), None)
                .expect_err("writing into a blocked namespace must fail")
        });

        assert!(
            error.to_string().contains("cache namespace dir"),
            "unexpected error: {error}"
        );
        assert!(
            logs.contains("Failed to create the cache namespace directory"),
            "a failed cache write must be logged; logs were:\n{logs}"
        );
    }

    #[test]
    fn test_generic_cache_properties() {
        let temp_dir = tempdir().unwrap();
        let cache = GenericCache::new(
            "test".to_string(),
            Some(temp_dir.path().to_str().unwrap().to_string()),
            30.0,
            500.0,
            1000.0,
        )
        .unwrap();

        assert_eq!(cache.cache_type(), "test");
        assert!(cache.cache_dir().to_string_lossy().contains("test"));
    }
}
