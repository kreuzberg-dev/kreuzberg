//! Benchmark configuration

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::types::DiskSizeInfo;
use crate::{Error, Result};

/// Benchmark execution mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BenchmarkMode {
    /// Single-file mode: Sequential execution (max_concurrent=1) for fair latency comparison
    SingleFile,
    /// Batch mode: Concurrent execution to measure throughput
    Batch,
}

/// CPU/memory profiling configuration for benchmark analysis
///
/// Controls adaptive sampling frequency, task duration amplification, and sample collection
/// thresholds to ensure high-quality profiles with 500-5000 samples per run.
///
/// # Sampling Frequency
///
/// The sampling frequency (100-10000 Hz) is automatically adjusted based on task duration:
/// - Quick tasks (<100ms): Higher frequency (up to 10000 Hz)
/// - Medium tasks (100-1000ms): Standard frequency (1000 Hz)
/// - Long tasks (>1000ms): Lower frequency (100-1000 Hz)
///
/// # Task Duration Amplification
///
/// When profiling is enabled, tasks can be amplified (repeated multiple times) to increase
/// profiling duration and reduce variance in sample collection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfilingConfig {
    /// Enable/disable CPU profiling
    pub enabled: bool,

    /// CPU sampling frequency in Hz (100-10000)
    /// Adjusted adaptively based on estimated task duration
    pub sampling_frequency: i32,

    /// Minimum task duration in milliseconds for adaptive frequency calculation
    /// Tasks shorter than this use higher sampling frequencies
    pub task_duration_ms: u64,

    /// Number of documents per profiling batch
    /// Larger batches provide more samples but increase memory usage
    pub batch_size: usize,

    /// Memory sample collection interval in milliseconds (0 = disabled)
    pub memory_sampling_interval_ms: u64,

    /// Enable flamegraph generation after profiling completes
    pub flamegraph_enabled: bool,

    /// Minimum number of samples required for a valid profile
    /// Profiles with fewer samples may have high variance
    pub sample_count_threshold: usize,
}

impl Default for ProfilingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            sampling_frequency: 1000,
            task_duration_ms: 500,
            batch_size: 10,
            memory_sampling_interval_ms: 10,
            flamegraph_enabled: true,
            sample_count_threshold: 500,
        }
    }
}

impl ProfilingConfig {
    /// Create a new profiling configuration with validation
    ///
    /// # Arguments
    ///
    /// * `sampling_frequency` - CPU sampling frequency in Hz (100-10000)
    /// * `batch_size` - Number of documents per profiling batch (must be > 0)
    /// * `sample_count_threshold` - Minimum samples for valid profile (must be > 0)
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::Config`] if any configuration value is invalid
    pub fn new(sampling_frequency: i32, batch_size: usize, sample_count_threshold: usize) -> crate::Result<Self> {
        let config = Self {
            enabled: false,
            sampling_frequency,
            task_duration_ms: 500,
            batch_size,
            memory_sampling_interval_ms: 10,
            flamegraph_enabled: true,
            sample_count_threshold,
        };
        config.validate()?;
        Ok(config)
    }

    /// Validate the profiling configuration
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::Config`] if any configuration value is invalid
    pub fn validate(&self) -> crate::Result<()> {
        if self.sampling_frequency < 100 || self.sampling_frequency > 10000 {
            return Err(crate::Error::Config(format!(
                "sampling_frequency must be 100-10000 Hz, got {}",
                self.sampling_frequency
            )));
        }

        if self.batch_size == 0 {
            return Err(crate::Error::Config("batch_size must be > 0".to_string()));
        }

        if self.sample_count_threshold == 0 {
            return Err(crate::Error::Config("sample_count_threshold must be > 0".to_string()));
        }

        Ok(())
    }

    /// Calculate optimal sampling frequency based on estimated task duration
    ///
    /// Uses realistic sysinfo limits (100-500 Hz) to achieve target sample count.
    /// sysinfo cannot reliably achieve >500 Hz on most systems due to:
    /// - Process scheduling granularity
    /// - System call overhead
    /// - File descriptor refresh costs
    ///
    /// Target: 500 samples minimum for statistical significance
    ///
    /// # Arguments
    ///
    /// * `estimated_duration_ms` - Estimated task duration in milliseconds
    ///
    /// # Returns
    ///
    /// Optimal sampling frequency in Hz (clamped to 100-500 range)
    pub fn calculate_optimal_frequency(estimated_duration_ms: u64) -> i32 {
        const TARGET_SAMPLE_COUNT: u64 = 500;
        const REALISTIC_MAX_HZ: i32 = 500;

        if estimated_duration_ms == 0 {
            return REALISTIC_MAX_HZ;
        }

        let required_hz = (TARGET_SAMPLE_COUNT * 1000) / estimated_duration_ms.max(1);
        (required_hz as i32).clamp(100, REALISTIC_MAX_HZ)
    }

    /// Calculate sampling interval in milliseconds from frequency in Hz
    ///
    /// Converts sampling frequency to the actual interval between samples.
    ///
    /// # Arguments
    ///
    /// * `sampling_frequency_hz` - Sampling frequency in Hz
    ///
    /// # Returns
    ///
    /// Sampling interval in milliseconds (minimum 1ms)
    pub fn calculate_sample_interval_ms(sampling_frequency_hz: i32) -> u64 {
        (1000 / sampling_frequency_hz as u64).max(1)
    }
}

/// Configuration for benchmark runs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkConfig {
    /// File types to include (e.g., ["pdf", "docx"])
    pub file_types: Option<Vec<String>>,

    /// Timeout for each extraction
    pub timeout: Duration,

    /// Maximum number of concurrent extractions
    pub max_concurrent: usize,

    /// Xberg's configured extraction thread budget.
    ///
    /// When unset, single-file extraction keeps Xberg's automatic budget and
    /// native batch extraction uses [`Self::max_concurrent`]. This setting
    /// does not affect other frameworks.
    #[serde(default)]
    pub xberg_max_threads: Option<usize>,

    /// Output directory for results
    pub output_dir: PathBuf,

    /// Whether to include quality assessment
    pub measure_quality: bool,

    /// Benchmark execution mode (single-file or batch)
    pub benchmark_mode: BenchmarkMode,

    /// Number of warmup iterations (discarded from statistics)
    pub warmup_iterations: usize,

    /// Number of benchmark iterations for statistical analysis
    pub benchmark_iterations: usize,

    /// Profiling configuration for CPU/memory analysis
    pub profiling: ProfilingConfig,

    /// Whether OCR is enabled for this benchmark run.
    /// When false, fixtures that require OCR (images, scanned PDFs) are excluded.
    pub ocr_enabled: bool,
}

impl Default for BenchmarkConfig {
    fn default() -> Self {
        Self {
            file_types: None,
            timeout: Duration::from_secs(1800),
            max_concurrent: num_cpus::get(),
            xberg_max_threads: None,
            output_dir: PathBuf::from("results"),
            measure_quality: false,
            benchmark_mode: BenchmarkMode::Batch,
            warmup_iterations: 1,
            benchmark_iterations: 3,
            profiling: ProfilingConfig::default(),
            ocr_enabled: false,
        }
    }
}

impl BenchmarkConfig {
    /// Create a new benchmark configuration with validation
    ///
    /// # Arguments
    ///
    /// * `output_dir` - Directory for results
    /// * `max_concurrent` - Maximum concurrent extractions (must be > 0)
    /// * `benchmark_iterations` - Number of iterations (must be > 0)
    /// * `timeout` - Timeout per extraction
    /// * `benchmark_mode` - SingleFile or Batch mode
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::Config`] if any configuration value is invalid
    pub fn new(
        output_dir: PathBuf,
        max_concurrent: usize,
        benchmark_iterations: usize,
        timeout: Duration,
        benchmark_mode: BenchmarkMode,
    ) -> crate::Result<Self> {
        let config = Self {
            file_types: None,
            timeout,
            max_concurrent,
            xberg_max_threads: None,
            output_dir,
            measure_quality: false,
            benchmark_mode,
            warmup_iterations: 1,
            benchmark_iterations,
            profiling: ProfilingConfig::default(),
            ocr_enabled: false,
        };
        config.validate()?;
        Ok(config)
    }

    /// Validate the configuration
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::Config`] if any configuration value is invalid
    pub fn validate(&self) -> crate::Result<()> {
        if self.timeout.as_secs() == 0 {
            return Err(crate::Error::Config("Timeout must be > 0".to_string()));
        }

        if self.max_concurrent == 0 {
            return Err(crate::Error::Config("max_concurrent must be > 0".to_string()));
        }

        if self.xberg_max_threads == Some(0) {
            return Err(crate::Error::Config("xberg_max_threads must be > 0".to_string()));
        }

        if self.benchmark_iterations == 0 {
            return Err(crate::Error::Config("benchmark_iterations must be > 0".to_string()));
        }

        if self.benchmark_mode == BenchmarkMode::SingleFile && self.max_concurrent != 1 {
            return Err(crate::Error::Config(
                "single-file mode requires max_concurrent=1".to_string(),
            ));
        }

        self.profiling.validate()?;

        Ok(())
    }
}

/// Load framework disk sizes from JSON configuration file
pub fn load_framework_sizes(config_path: &Path) -> Result<HashMap<String, DiskSizeInfo>> {
    let json_content = std::fs::read_to_string(config_path).map_err(Error::Io)?;

    let sizes: HashMap<String, DiskSizeInfo> = serde_json::from_str(&json_content)
        .map_err(|e| Error::Benchmark(format!("Failed to parse framework sizes: {}", e)))?;

    Ok(sizes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_batch_config() {
        let config = BenchmarkConfig::new(
            PathBuf::from("/tmp/results"),
            4,
            3,
            Duration::from_secs(180),
            BenchmarkMode::Batch,
        );
        assert!(config.is_ok());
    }

    #[test]
    fn test_valid_single_file_config() {
        let config = BenchmarkConfig::new(
            PathBuf::from("/tmp/results"),
            1,
            3,
            Duration::from_secs(180),
            BenchmarkMode::SingleFile,
        );
        assert!(config.is_ok());
    }

    #[test]
    fn test_zero_timeout_rejected() {
        let config = BenchmarkConfig::new(
            PathBuf::from("/tmp/results"),
            4,
            3,
            Duration::from_secs(0),
            BenchmarkMode::Batch,
        );
        assert!(config.is_err());
        let msg = format!("{}", config.unwrap_err());
        assert!(msg.contains("Timeout must be > 0"));
    }

    #[test]
    fn test_zero_max_concurrent_rejected() {
        let config = BenchmarkConfig::new(
            PathBuf::from("/tmp/results"),
            0,
            3,
            Duration::from_secs(180),
            BenchmarkMode::Batch,
        );
        assert!(config.is_err());
        let msg = format!("{}", config.unwrap_err());
        assert!(msg.contains("max_concurrent must be > 0"));
    }

    #[test]
    fn test_zero_xberg_max_threads_rejected() {
        let config = BenchmarkConfig {
            xberg_max_threads: Some(0),
            ..Default::default()
        };

        let error = config.validate().unwrap_err();
        assert!(error.to_string().contains("xberg_max_threads must be > 0"));
    }

    #[test]
    fn test_xberg_max_threads_defaults_to_legacy_behavior() {
        let config = BenchmarkConfig::default();
        assert_eq!(config.xberg_max_threads, None);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_legacy_config_deserializes_without_xberg_max_threads() {
        let mut legacy = serde_json::to_value(BenchmarkConfig::default()).unwrap();
        legacy.as_object_mut().unwrap().remove("xberg_max_threads");

        let config: BenchmarkConfig = serde_json::from_value(legacy).unwrap();
        assert_eq!(config.xberg_max_threads, None);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_zero_iterations_rejected() {
        let config = BenchmarkConfig::new(
            PathBuf::from("/tmp/results"),
            4,
            0,
            Duration::from_secs(180),
            BenchmarkMode::Batch,
        );
        assert!(config.is_err());
        let msg = format!("{}", config.unwrap_err());
        assert!(msg.contains("benchmark_iterations must be > 0"));
    }

    #[test]
    fn test_single_file_mode_requires_max_concurrent_one() {
        let config = BenchmarkConfig::new(
            PathBuf::from("/tmp/results"),
            4,
            3,
            Duration::from_secs(180),
            BenchmarkMode::SingleFile,
        );
        assert!(config.is_err());
        let msg = format!("{}", config.unwrap_err());
        assert!(msg.contains("single-file mode requires max_concurrent=1"));
    }

    #[test]
    fn test_default_config_validates() {
        let config = BenchmarkConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_valid_profiling_config() {
        let config = ProfilingConfig::new(1000, 10, 500);
        assert!(config.is_ok());
    }

    #[test]
    fn test_profiling_frequency_too_low() {
        let config = ProfilingConfig::new(50, 10, 500);
        assert!(config.is_err());
        let msg = format!("{}", config.unwrap_err());
        assert!(msg.contains("sampling_frequency must be 100-10000 Hz"));
    }

    #[test]
    fn test_profiling_frequency_too_high() {
        let config = ProfilingConfig::new(20_000, 10, 500);
        assert!(config.is_err());
        let msg = format!("{}", config.unwrap_err());
        assert!(msg.contains("sampling_frequency must be 100-10000 Hz"));
    }

    #[test]
    fn test_profiling_zero_batch_size() {
        let config = ProfilingConfig::new(1000, 0, 500);
        assert!(config.is_err());
        let msg = format!("{}", config.unwrap_err());
        assert!(msg.contains("batch_size must be > 0"));
    }

    #[test]
    fn test_profiling_zero_sample_threshold() {
        let config = ProfilingConfig::new(1000, 10, 0);
        assert!(config.is_err());
        let msg = format!("{}", config.unwrap_err());
        assert!(msg.contains("sample_count_threshold must be > 0"));
    }

    #[test]
    fn test_profiling_boundary_frequencies() {
        assert!(ProfilingConfig::new(100, 1, 1).is_ok());
        assert!(ProfilingConfig::new(10000, 1, 1).is_ok());
        assert!(ProfilingConfig::new(99, 1, 1).is_err());
        assert!(ProfilingConfig::new(10001, 1, 1).is_err());
    }

    #[test]
    fn test_optimal_frequency_zero_duration() {
        let freq = ProfilingConfig::calculate_optimal_frequency(0);
        assert_eq!(freq, 500);
    }

    #[test]
    fn test_optimal_frequency_short_task() {
        let freq = ProfilingConfig::calculate_optimal_frequency(100);
        assert_eq!(freq, 500);
    }

    #[test]
    fn test_optimal_frequency_long_task() {
        let freq = ProfilingConfig::calculate_optimal_frequency(10_000);
        assert_eq!(freq, 100);
    }

    #[test]
    fn test_sample_interval_calculation() {
        assert_eq!(ProfilingConfig::calculate_sample_interval_ms(1000), 1);
        assert_eq!(ProfilingConfig::calculate_sample_interval_ms(100), 10);
        assert_eq!(ProfilingConfig::calculate_sample_interval_ms(500), 2);
    }
}
