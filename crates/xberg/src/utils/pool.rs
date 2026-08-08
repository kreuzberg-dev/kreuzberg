//! Object pooling utilities for reducing allocations in batch processing.
//!
//! This module provides reusable object pools to reduce garbage collection and allocator
//! pressure during batch document extraction. Instead of creating/destroying temporary objects
//! repeatedly, pools maintain a collection of pre-allocated objects that can be quickly reused.
//!
//! # Performance Benefits
//!
//! For batch extraction of N documents:
//! - Without pooling: N allocations + N deallocations per run
//! - With pooling: 1-2 allocations per run (pool creation), zero during document processing
//!
//! Expected improvement: 5-10% overall throughput improvement on batch operations.
//!
//! # Thread Safety
//!
//! All pools are `Send + Sync` and can be safely shared across threads using `Arc<Pool<T>>`.
//! Internal state is protected by `Mutex` for proper synchronization.
//!
//! # Example
//!
//! Pools are constructed through the public factory functions; the `Pool::new`
//! constructor itself is crate-internal.
//!
//! ```rust,no_run
//! use xberg::utils::pool::create_string_buffer_pool;
//!
//! let pool = create_string_buffer_pool(10, 4096); // 10 buffers of 4KB each
//! let mut buffer = pool.acquire();
//! buffer.push_str("some content");
//! // buffer is returned to pool when dropped
//! ```

use parking_lot::Mutex;
use std::sync::Arc;

#[cfg(feature = "pool-metrics")]
use std::sync::atomic::{AtomicUsize, Ordering};

/// Metrics tracking for pool allocations and reuse patterns.
///
/// These metrics help identify pool efficiency and allocation patterns.
/// Only available when the `pool-metrics` feature is enabled.
#[cfg(feature = "pool-metrics")]
#[cfg_attr(alef, alef(skip))]
#[derive(Debug)]
pub struct PoolMetrics {
    /// Total number of acquire calls on this pool
    pub total_acquires: AtomicUsize,
    /// Total number of cache hits (reused objects from pool)
    pub total_cache_hits: AtomicUsize,
    /// Peak number of objects stored simultaneously in this pool
    pub peak_items_stored: AtomicUsize,
    /// Total number of objects created by the factory function
    pub total_creations: AtomicUsize,
}

#[cfg(feature = "pool-metrics")]
impl PoolMetrics {
    /// Create a new metrics tracker with all counters at zero.
    pub(crate) fn new() -> Self {
        PoolMetrics {
            total_acquires: AtomicUsize::new(0),
            total_cache_hits: AtomicUsize::new(0),
            peak_items_stored: AtomicUsize::new(0),
            total_creations: AtomicUsize::new(0),
        }
    }

    /// Calculate the cache hit rate as a percentage (0.0-100.0).
    #[cfg(test)]
    pub(crate) fn hit_rate(&self) -> f64 {
        let acquires = self.total_acquires.load(Ordering::Relaxed);
        if acquires == 0 {
            return 0.0;
        }
        (self.total_cache_hits.load(Ordering::Relaxed) as f64 / acquires as f64) * 100.0
    }
}

#[cfg(feature = "pool-metrics")]
impl Default for PoolMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// A thread-safe object pool that reuses instances to reduce allocations.
///
/// Generic over any type that implements `Recyclable`, allowing pooling of
/// different object types with custom reset logic.
#[doc(hidden)]
#[cfg_attr(alef, alef(skip))]
#[derive(Clone)]
pub struct Pool<T: Recyclable> {
    factory: Arc<dyn Fn() -> T + Send + Sync>,
    objects: Arc<Mutex<Vec<T>>>,
    max_size: usize,
    #[cfg(feature = "pool-metrics")]
    metrics: Arc<PoolMetrics>,
}

/// Trait for types that can be pooled and reused.
///
/// Implementing this trait allows a type to be used with `Pool<T>`.
/// The `reset()` method should clear the object's state for reuse.
#[doc(hidden)]
#[cfg_attr(alef, alef(skip))]
pub trait Recyclable: Send + 'static {
    /// Reset the object to a reusable state.
    ///
    /// This is called when returning an object to the pool.
    /// Should clear any internal data while preserving capacity.
    fn reset(&mut self);
}

impl<T: Recyclable> Pool<T> {
    /// Create a new pool with a given factory and maximum size.
    ///
    /// # Arguments
    ///
    /// * `factory` - Function that creates new instances
    /// * `max_size` - Maximum number of objects to keep in the pool
    ///
    /// # Returns
    ///
    /// A new pool ready to use.
    ///
    /// # Example
    ///
    /// This constructor is crate-internal. Callers outside the crate build pools
    /// through [`create_string_buffer_pool`] / [`create_byte_buffer_pool`], which
    /// wrap it:
    ///
    /// ```rust,no_run
    /// use xberg::utils::pool::create_string_buffer_pool;
    ///
    /// let pool = create_string_buffer_pool(10, 0);
    /// ```
    pub(crate) fn new<F>(factory: F, max_size: usize) -> Self
    where
        F: Fn() -> T + Send + Sync + 'static,
    {
        Pool {
            factory: Arc::new(factory),
            objects: Arc::new(Mutex::new(Vec::with_capacity(max_size))),
            max_size,
            #[cfg(feature = "pool-metrics")]
            metrics: Arc::new(PoolMetrics::new()),
        }
    }

    /// Acquire an object from the pool or create a new one if empty.
    ///
    /// # Returns
    ///
    /// A `PoolGuard<T>` that will return the object to the pool when dropped.
    ///
    /// # Panics
    ///
    /// Panics if the mutex is already locked by the current thread (deadlock).
    /// This is a safety mechanism provided by parking_lot to prevent subtle bugs.
    pub fn acquire(&self) -> PoolGuard<T> {
        #[cfg(feature = "pool-metrics")]
        self.metrics.total_acquires.fetch_add(1, Ordering::Relaxed);

        let mut objects = self.objects.lock();

        let object = if let Some(mut obj) = objects.pop() {
            #[cfg(feature = "pool-metrics")]
            self.metrics.total_cache_hits.fetch_add(1, Ordering::Relaxed);

            obj.reset();
            obj
        } else {
            #[cfg(feature = "pool-metrics")]
            self.metrics.total_creations.fetch_add(1, Ordering::Relaxed);

            (self.factory)()
        };

        PoolGuard {
            object: Some(object),
            pool: Pool {
                factory: Arc::clone(&self.factory),
                objects: Arc::clone(&self.objects),
                max_size: self.max_size,
                #[cfg(feature = "pool-metrics")]
                metrics: Arc::clone(&self.metrics),
            },
        }
    }

    /// Get the current number of objects in the pool.
    pub fn size(&self) -> usize {
        self.objects.lock().len()
    }

    /// Clear the pool, discarding all pooled objects.
    #[cfg(any(feature = "tokio-runtime", test))]
    pub(crate) fn clear(&self) {
        self.objects.lock().clear();
    }

    /// Get a reference to the pool metrics (only available with `pool-metrics` feature).
    #[cfg(all(feature = "pool-metrics", test))]
    pub(crate) fn metrics(&self) -> &PoolMetrics {
        &self.metrics
    }
}

/// RAII guard that returns an object to the pool when dropped.
#[doc(hidden)]
pub struct PoolGuard<T: Recyclable> {
    object: Option<T>,
    pool: Pool<T>,
}

impl<T: Recyclable> std::ops::Deref for PoolGuard<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.object.as_ref().expect("object should never be None")
    }
}

impl<T: Recyclable> std::ops::DerefMut for PoolGuard<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.object.as_mut().expect("object should never be None")
    }
}

impl<T: Recyclable> Drop for PoolGuard<T> {
    fn drop(&mut self) {
        if let Some(object) = self.object.take() {
            let mut objects = self.pool.objects.lock();
            if objects.len() < self.pool.max_size {
                objects.push(object);

                #[cfg(feature = "pool-metrics")]
                {
                    let current_size = objects.len();
                    let peak = self.pool.metrics.peak_items_stored.load(Ordering::Relaxed);
                    if current_size > peak {
                        self.pool
                            .metrics
                            .peak_items_stored
                            .store(current_size, Ordering::Relaxed);
                    }
                }
            }
        }
    }
}

/// Pooled string buffer for text accumulation.
///
/// Reuses a single String allocation across multiple uses to reduce allocations
/// when processing multiple documents.
impl Recyclable for String {
    fn reset(&mut self) {
        self.clear();
    }
}

/// Pooled byte vector for binary data handling.
///
/// Reuses a Vec<u8> allocation across multiple uses.
impl Recyclable for Vec<u8> {
    fn reset(&mut self) {
        self.clear();
    }
}

/// Convenience type alias for a pooled String.
#[doc(hidden)]
#[cfg_attr(alef, alef(skip))]
pub type StringBufferPool = Pool<String>;

/// Convenience type alias for a pooled Vec<u8>.
#[doc(hidden)]
#[cfg_attr(alef, alef(skip))]
pub type ByteBufferPool = Pool<Vec<u8>>;

/// Create a pre-configured string buffer pool for batch processing.
///
/// # Arguments
///
/// * `pool_size` - Maximum number of buffers to keep in the pool
/// * `buffer_capacity` - Initial capacity for each buffer in bytes
///
/// # Returns
///
/// A pool configured for text accumulation with reasonable defaults.
///
/// # Example
///
/// ```rust,no_run
/// use xberg::utils::pool::create_string_buffer_pool;
///
/// let pool = create_string_buffer_pool(10, 8192);
/// let mut buffer = pool.acquire();
/// buffer.push_str("content");
/// ```
#[cfg_attr(alef, alef(skip))]
pub fn create_string_buffer_pool(pool_size: usize, buffer_capacity: usize) -> StringBufferPool {
    Pool::new(move || String::with_capacity(buffer_capacity), pool_size)
}

/// Create a pre-configured byte buffer pool for batch processing.
///
/// # Arguments
///
/// * `pool_size` - Maximum number of buffers to keep in the pool
/// * `buffer_capacity` - Initial capacity for each buffer in bytes
///
/// # Returns
///
/// A pool configured for binary data handling with reasonable defaults.
///
/// # Example
///
/// ```rust,no_run
/// use xberg::utils::pool::create_byte_buffer_pool;
///
/// let pool = create_byte_buffer_pool(10, 65536);
/// let mut buffer = pool.acquire();
/// buffer.extend_from_slice(b"binary data");
/// ```
#[cfg_attr(alef, alef(skip))]
pub fn create_byte_buffer_pool(pool_size: usize, buffer_capacity: usize) -> ByteBufferPool {
    Pool::new(move || Vec::with_capacity(buffer_capacity), pool_size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_acquire_and_reuse() {
        let pool = Pool::new(String::new, 5);

        {
            let mut s1 = pool.acquire();
            s1.push_str("hello");
            assert_eq!(s1.len(), 5);
        }

        {
            let s2 = pool.acquire();
            assert_eq!(s2.len(), 0, "string should be cleared when reused");
        }
    }

    #[test]
    fn test_pool_respects_max_size() {
        let pool = Pool::new(String::new, 2);

        let guard1 = pool.acquire();
        let guard2 = pool.acquire();
        let guard3 = pool.acquire();

        drop(guard1);
        drop(guard2);
        drop(guard3);

        assert!(pool.size() <= 2, "pool size should not exceed max_size");
    }

    #[test]
    fn test_pool_clear() {
        let pool = Pool::new(String::new, 5);

        let _g1 = pool.acquire();
        let _g2 = pool.acquire();

        drop(_g1);
        drop(_g2);

        assert!(pool.size() > 0, "pool should have items");
        pool.clear();
        assert_eq!(pool.size(), 0, "pool should be empty after clear");
    }

    #[test]
    fn test_byte_buffer_pool() {
        let pool = Pool::new(Vec::new, 5);

        {
            let mut buf = pool.acquire();
            buf.extend_from_slice(b"hello");
            assert_eq!(buf.len(), 5);
        }

        {
            let buf = pool.acquire();
            assert_eq!(buf.len(), 0, "buffer should be cleared");
        }
    }

    #[test]
    fn test_pool_deref() {
        let pool = Pool::new(String::new, 5);
        let mut guard = pool.acquire();

        let _len = guard.len();

        guard.push_str("test");
        assert_eq!(&*guard, "test");
    }

    #[test]
    fn test_concurrent_access() {
        use std::sync::Arc;
        use std::thread;

        let pool = Arc::new(Pool::new(String::new, 10));
        let mut handles = vec![];

        for i in 0..5 {
            let pool_clone = Arc::clone(&pool);
            let handle = thread::spawn(move || {
                let mut buf = pool_clone.acquire();
                buf.push_str(&format!("thread_{}", i));
                std::thread::sleep(std::time::Duration::from_millis(10));
                drop(buf);
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().unwrap();
        }

        assert!(pool.size() <= 10);
    }

    #[test]
    #[cfg(feature = "pool-metrics")]
    fn test_pool_metrics_tracking() {
        let pool = Pool::new(String::new, 5);

        {
            let _s1 = pool.acquire();
        }

        {
            let _s2 = pool.acquire();
        }

        let metrics = pool.metrics();
        assert_eq!(metrics.total_acquires.load(Ordering::Relaxed), 2);
        assert_eq!(metrics.total_cache_hits.load(Ordering::Relaxed), 1);
        assert_eq!(metrics.total_creations.load(Ordering::Relaxed), 1);

        let hit_rate = metrics.hit_rate();
        assert!(hit_rate > 49.0 && hit_rate < 51.0);
    }

    #[test]
    #[cfg(feature = "pool-metrics")]
    fn test_pool_metrics_peak_tracking() {
        let pool = Pool::new(String::new, 5);

        let g1 = pool.acquire();
        let g2 = pool.acquire();
        let g3 = pool.acquire();

        drop(g1);
        drop(g2);
        drop(g3);

        let metrics = pool.metrics();
        assert!(metrics.peak_items_stored.load(Ordering::Relaxed) >= 3);
    }
}
