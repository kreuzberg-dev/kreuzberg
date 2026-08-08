//! Security utilities for document extractors.
//!
//! This module provides validation and protection mechanisms against common attacks:
//! - ZIP bomb detection (decompression bombs)
//! - XML entity expansion limits
//! - Nesting depth limits
//! - Input size limits
//! - Entity length validation
//! - Path traversal detection

#[cfg(any(feature = "archives", feature = "hwpx", feature = "iwork"))]
use std::io::{Read, Seek};

/// Configuration for security limits across extractors.
///
/// All limits are intentionally conservative to prevent DoS attacks
/// while still supporting legitimate documents.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
#[serde(default)]
pub struct SecurityLimits {
    /// Maximum uncompressed size for archives (500 MB)
    pub max_archive_size: usize,

    /// Maximum compression ratio before flagging as potential bomb (100:1)
    pub max_compression_ratio: usize,

    /// Maximum number of files in archive (10,000)
    pub max_files_in_archive: usize,

    /// Maximum nesting depth for structures (100)
    pub max_nesting_depth: usize,

    /// Maximum length of any single XML entity / attribute / token (1 MiB).
    /// This is a per-token cap, NOT a total cap — billion-laughs class
    /// attacks where a single entity expands to hundreds of MB are caught
    /// here, while normal long text content (a paragraph, a CDATA block) is
    /// caught by `max_content_size` instead.
    pub max_entity_length: usize,

    /// Maximum string growth per document (100 MB)
    pub max_content_size: usize,

    /// Maximum iterations per operation
    pub max_iterations: usize,

    /// Maximum XML depth (100 levels)
    pub max_xml_depth: usize,

    /// Maximum cells per table (100,000)
    pub max_table_cells: usize,
}

impl Default for SecurityLimits {
    fn default() -> Self {
        Self {
            max_archive_size: 500 * 1024 * 1024,
            max_compression_ratio: 100,
            max_files_in_archive: 10_000,
            max_nesting_depth: 1024,
            max_entity_length: 1024 * 1024,
            max_content_size: 100 * 1024 * 1024,
            max_iterations: 10_000_000,
            max_xml_depth: 1024,
            max_table_cells: 100_000,
        }
    }
}

/// Security validation errors.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone)]
pub enum SecurityError {
    /// Potential ZIP bomb detected
    ZipBombDetected {
        /// Compressed size in bytes.
        compressed_size: u64,
        /// Uncompressed size in bytes.
        uncompressed_size: u64,
        /// Observed compression ratio (uncompressed / compressed).
        ratio: f64,
    },

    /// Archive exceeds maximum size
    ArchiveTooLarge {
        /// Total uncompressed size in bytes.
        size: u64,
        /// Configured maximum in bytes.
        max: usize,
    },

    /// Archive contains too many files
    TooManyFiles {
        /// Number of files found in the archive.
        count: usize,
        /// Configured maximum file count.
        max: usize,
    },

    /// Nesting too deep
    NestingTooDeep {
        /// Current nesting depth reached.
        depth: usize,
        /// Configured maximum depth.
        max: usize,
    },

    /// Content exceeds maximum size
    ContentTooLarge {
        /// Accumulated content size in bytes.
        size: usize,
        /// Configured maximum in bytes.
        max: usize,
    },

    /// Entity/string too long
    EntityTooLong {
        /// Length of the offending entity in bytes.
        length: usize,
        /// Configured maximum entity length in bytes.
        max: usize,
    },

    /// Too many iterations
    TooManyIterations {
        /// Current iteration count.
        count: usize,
        /// Configured maximum iteration count.
        max: usize,
    },

    /// XML depth exceeded
    XmlDepthExceeded {
        /// Current XML element depth.
        depth: usize,
        /// Configured maximum XML depth.
        max: usize,
    },

    /// Too many table cells
    TooManyCells {
        /// Accumulated cell count.
        cells: usize,
        /// Configured maximum cell count.
        max: usize,
    },

    /// An archive entry could not be read, so its declared sizes could not be
    /// counted towards the archive limits. Reported rather than skipped: an
    /// unaccounted entry makes every aggregate total untrustworthy.
    UnreadableEntry {
        /// Zero-based index of the entry in the archive's central directory.
        index: usize,
        /// Why the entry header could not be read.
        reason: String,
    },
}

impl std::fmt::Display for SecurityError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            SecurityError::ZipBombDetected {
                compressed_size,
                uncompressed_size,
                ratio,
            } => {
                write!(
                    f,
                    "Potential ZIP bomb detected: compressed {}B -> uncompressed {}B (ratio: {:.1}:1)",
                    compressed_size, uncompressed_size, ratio
                )
            }
            SecurityError::ArchiveTooLarge { size, max } => {
                write!(f, "Archive too large: {} bytes (max: {} bytes)", size, max)
            }
            SecurityError::TooManyFiles { count, max } => {
                write!(f, "Archive has too many files: {} (max: {})", count, max)
            }
            SecurityError::NestingTooDeep { depth, max } => {
                write!(f, "Nesting too deep: {} levels (max: {})", depth, max)
            }
            SecurityError::ContentTooLarge { size, max } => {
                write!(f, "Content too large: {} bytes (max: {} bytes)", size, max)
            }
            SecurityError::EntityTooLong { length, max } => {
                write!(f, "Entity too long: {} chars (max: {})", length, max)
            }
            SecurityError::TooManyIterations { count, max } => {
                write!(f, "Too many iterations: {} (max: {})", count, max)
            }
            SecurityError::XmlDepthExceeded { depth, max } => {
                write!(f, "XML depth exceeded: {} (max: {})", depth, max)
            }
            SecurityError::TooManyCells { cells, max } => {
                write!(f, "Too many table cells: {} (max: {})", cells, max)
            }
            SecurityError::UnreadableEntry { index, reason } => {
                write!(
                    f,
                    "Archive entry {} could not be read for security accounting: {}",
                    index, reason
                )
            }
        }
    }
}

impl std::error::Error for SecurityError {}

/// Helper struct for validating ZIP archives for security issues.
#[cfg(any(feature = "archives", feature = "hwpx", feature = "iwork"))]
#[cfg_attr(alef, alef(skip))]
pub struct ZipBombValidator {
    limits: SecurityLimits,
}

#[cfg(any(feature = "archives", feature = "hwpx", feature = "iwork"))]
impl ZipBombValidator {
    /// Create a new ZIP bomb validator.
    pub(crate) fn new(limits: SecurityLimits) -> Self {
        Self { limits }
    }

    /// Validate a ZIP archive for security issues.
    ///
    /// Every entry listed in the central directory is accounted for. Sizes are read via
    /// `zip::ZipArchive::by_index_raw`, which parses the entry header without building a
    /// decompressor, so entries using an unsupported compression method or requiring a
    /// password still contribute to the totals instead of dropping out of them. An entry
    /// whose header cannot be read at all is reported as `SecurityError::UnreadableEntry`
    /// rather than skipped: an unaccounted entry means the aggregate totals below no longer
    /// bound what extraction will do.
    ///
    /// Accumulation uses saturating arithmetic and the running total is compared against
    /// `max_archive_size` after *every* entry. Declared sizes come straight from attacker
    /// controlled ZIP64 headers and can each be close to `u64::MAX`, so an unchecked `+=`
    /// would wrap the total back down to a small value and let the archive through.
    ///
    /// # Arguments
    /// * `archive` - Mutable ZIP archive to validate
    ///
    /// # Returns
    /// * `Ok(())` if archive is safe
    /// * `Err(SecurityError)` if security limit violated
    pub(crate) fn validate<R: Read + Seek>(&self, archive: &mut zip::ZipArchive<R>) -> Result<(), SecurityError> {
        let file_count = archive.len();

        if file_count > self.limits.max_files_in_archive {
            return Err(SecurityError::TooManyFiles {
                count: file_count,
                max: self.limits.max_files_in_archive,
            });
        }

        let max_archive_size = self.limits.max_archive_size as u64;
        let max_compression_ratio = self.limits.max_compression_ratio as f64;
        let mut total_uncompressed: u64 = 0;
        let mut total_compressed: u64 = 0;

        for index in 0..file_count {
            let (compressed_size, uncompressed_size) = match archive.by_index_raw(index) {
                Ok(file) => (file.compressed_size(), file.size()),
                Err(error) => {
                    return Err(SecurityError::UnreadableEntry {
                        index,
                        reason: error.to_string(),
                    });
                }
            };

            total_uncompressed = total_uncompressed.saturating_add(uncompressed_size);
            total_compressed = total_compressed.saturating_add(compressed_size);

            if uncompressed_size > 0 {
                // A zero compressed size paired with a non-zero uncompressed size cannot be
                // produced by any compressor; treating it as an unbounded ratio stops the
                // entry from slipping past this check on a division it never performs. ~keep
                let ratio = if compressed_size == 0 {
                    f64::INFINITY
                } else {
                    uncompressed_size as f64 / compressed_size as f64
                };
                if ratio > max_compression_ratio {
                    return Err(SecurityError::ZipBombDetected {
                        compressed_size,
                        uncompressed_size,
                        ratio,
                    });
                }
            }

            if total_uncompressed > max_archive_size {
                return Err(SecurityError::ArchiveTooLarge {
                    size: total_uncompressed,
                    max: self.limits.max_archive_size,
                });
            }
        }

        if total_compressed > 0 {
            let ratio = total_uncompressed as f64 / total_compressed as f64;
            if ratio > max_compression_ratio {
                return Err(SecurityError::ZipBombDetected {
                    compressed_size: total_compressed,
                    uncompressed_size: total_uncompressed,
                    ratio,
                });
            }
        }

        Ok(())
    }
}

/// Helper struct for tracking and validating aggregate string growth during extraction.
///
/// Use this when an extractor accumulates user-controlled content into a `String`
/// or `Vec<u8>`. Call `check_append(len)` *before* pushing each chunk so the producer
/// can stop early on a quadratic-concatenation / billion-laughs-style attack instead
/// of OOMing the process.
///
/// `Send + Sync` because all state is owned and contains only primitives.
#[derive(Debug, Clone)]
pub(crate) struct StringGrowthValidator {
    max_size: usize,
    current_size: usize,
}

impl StringGrowthValidator {
    /// Create a new string growth validator capped at `max_size` bytes.
    pub(crate) fn new(max_size: usize) -> Self {
        Self {
            max_size,
            current_size: 0,
        }
    }

    /// Account for `len` more bytes about to be appended.
    ///
    /// Returns `Err(SecurityError::ContentTooLarge)` when the running total exceeds
    /// `max_size`. Counter is updated using saturating arithmetic so a malicious caller
    /// cannot wrap to zero.
    pub(crate) fn check_append(&mut self, len: usize) -> Result<(), SecurityError> {
        self.current_size = self.current_size.saturating_add(len);
        if self.current_size > self.max_size {
            Err(SecurityError::ContentTooLarge {
                size: self.current_size,
                max: self.max_size,
            })
        } else {
            Ok(())
        }
    }
}

/// Helper struct for capping iteration counts in parser loops.
///
/// Use inside any unbounded loop reading a user-controlled stream
/// (XML token loop, HTML tokenizer, JSON parser) to bail out before a malicious
/// document spins the CPU. Call `check_iteration()` once per loop turn.
#[derive(Debug, Clone)]
pub(crate) struct IterationValidator {
    max_iterations: usize,
    current_count: usize,
}

impl IterationValidator {
    /// Create a new iteration validator capped at `max_iterations`.
    pub(crate) fn new(max_iterations: usize) -> Self {
        Self {
            max_iterations,
            current_count: 0,
        }
    }

    /// Increment the counter and return `Err(SecurityError::TooManyIterations)`
    /// once `max_iterations` is exceeded.
    pub(crate) fn check_iteration(&mut self) -> Result<(), SecurityError> {
        self.current_count = self.current_count.saturating_add(1);
        if self.current_count > self.max_iterations {
            Err(SecurityError::TooManyIterations {
                count: self.current_count,
                max: self.max_iterations,
            })
        } else {
            Ok(())
        }
    }
}

/// Helper struct for capping recursion / nesting depth.
///
/// Use to bound XML element nesting, HTML DOM depth, JSON object nesting, etc.
/// `push()` increments before checking so the *cap* depth itself is allowed
/// (e.g. `max_depth=100` accepts depth 100 and rejects 101). Always pair with
/// `pop()` on the matching close event.
#[derive(Debug, Clone)]
pub(crate) struct DepthValidator {
    max_depth: usize,
    current_depth: usize,
}

impl DepthValidator {
    /// Create a new depth validator capped at `max_depth` levels.
    pub(crate) fn new(max_depth: usize) -> Self {
        Self {
            max_depth,
            current_depth: 0,
        }
    }

    /// Enter one level of nesting. Returns `Err(SecurityError::NestingTooDeep)`
    /// once depth exceeds `max_depth`.
    pub(crate) fn push(&mut self) -> Result<(), SecurityError> {
        self.current_depth = self.current_depth.saturating_add(1);
        if self.current_depth > self.max_depth {
            Err(SecurityError::NestingTooDeep {
                depth: self.current_depth,
                max: self.max_depth,
            })
        } else {
            Ok(())
        }
    }

    /// Exit one level of nesting. Saturates at zero so an unbalanced close
    /// event in a malformed document cannot underflow.
    pub(crate) fn pop(&mut self) {
        if self.current_depth > 0 {
            self.current_depth -= 1;
        }
    }
}

/// Helper struct for capping individual entity / attribute string length.
///
/// Use against XML entity expansion (billion-laughs class) and any place
/// a single token can grow unboundedly. Stateless — safe to share by reference
/// across an extraction.
#[derive(Debug, Clone, Copy)]
pub(crate) struct EntityValidator {
    max_length: usize,
}

impl EntityValidator {
    /// Create a new entity validator capped at `max_length` bytes.
    pub(crate) fn new(max_length: usize) -> Self {
        Self { max_length }
    }

    /// Validate that `content` does not exceed `max_length`.
    pub(crate) fn validate(&self, content: &str) -> Result<(), SecurityError> {
        if content.len() > self.max_length {
            Err(SecurityError::EntityTooLong {
                length: content.len(),
                max: self.max_length,
            })
        } else {
            Ok(())
        }
    }

    /// Validate an XML attribute name+value pair. The check is applied to the
    /// value (attribute names are normally short) but the name is included in
    /// the call signature so callers can wire `quick_xml::Reader` attribute
    /// iteration directly.
    #[cfg(any(feature = "xml", feature = "office"))]
    pub(crate) fn check_attr(&self, _name: &str, value: &str) -> Result<(), SecurityError> {
        self.validate(value)
    }
}

/// Helper struct for capping aggregate table-cell counts across a document.
///
/// Use in CSV/XLSX/HTML table extraction to prevent a malicious document
/// from claiming billions of empty cells and exhausting memory. Call
/// `add_cells(n)` once per row (or once per emitted batch); the validator
/// fails when the running total exceeds `max_cells`.
#[derive(Debug, Clone)]
pub(crate) struct TableValidator {
    max_cells: usize,
    current_cells: usize,
}

impl TableValidator {
    /// Create a new table validator capped at `max_cells` total cells.
    pub(crate) fn new(max_cells: usize) -> Self {
        Self {
            max_cells,
            current_cells: 0,
        }
    }

    /// Account for `count` more cells. Returns `Err(SecurityError::TooManyCells)`
    /// once the running total exceeds `max_cells`. Saturating arithmetic.
    pub(crate) fn add_cells(&mut self, count: usize) -> Result<(), SecurityError> {
        self.current_cells = self.current_cells.saturating_add(count);
        if self.current_cells > self.max_cells {
            Err(SecurityError::TooManyCells {
                cells: self.current_cells,
                max: self.max_cells,
            })
        } else {
            Ok(())
        }
    }
}

/// Bundle of the four hostile-input validators tied to a single document
/// extraction. Holds running counters (depth, iteration, content size) plus
/// the stateless entity-length checker, so a single mutable reference threaded
/// into a parser is enough to enforce every limit advertised by `SecurityLimits`.
///
/// The convenience constructors build the bundle from either a borrowed
/// `SecurityLimits` or an `ExtractionConfig` (taking the `security_limits`
/// override, falling back to defaults when `None`).
#[derive(Debug, Clone)]
pub(crate) struct SecurityBudget {
    pub(crate) depth: DepthValidator,
    pub(crate) iteration: IterationValidator,
    pub(crate) entity: EntityValidator,
    pub(crate) growth: StringGrowthValidator,
    /// Cell counter for tabular extraction (CSV, XLSX, HTML tables).
    /// Threaded alongside the per-event budget but only consumed by table-emitting paths.
    pub(crate) table: TableValidator,
}

impl SecurityBudget {
    /// Build a budget from a borrowed `SecurityLimits`.
    pub(crate) fn from_limits(limits: &SecurityLimits) -> Self {
        Self {
            // Both limits apply to the same parse, so the budget must honour the tighter
            // of the two. Taking the looser value silently discards a caller's attempt to
            // clamp nesting via either knob. ~keep
            depth: DepthValidator::new(limits.max_xml_depth.min(limits.max_nesting_depth)),
            iteration: IterationValidator::new(limits.max_iterations),
            entity: EntityValidator::new(limits.max_entity_length),
            growth: StringGrowthValidator::new(limits.max_content_size),
            table: TableValidator::new(limits.max_table_cells),
        }
    }

    /// Build a protobuf/iWork budget using the format-agnostic nesting limit.
    // All callers live in the `iwork`-gated extractor module, so gate the
    // constructor to match — otherwise it is dead code under feature combos
    // that omit `iwork` (e.g. the no-ORT tract clippy leg). ~keep
    #[cfg(feature = "iwork")]
    pub(crate) fn for_iwork(limits: &SecurityLimits) -> Self {
        let mut budget = Self::from_limits(limits);
        // iWork parses protobuf messages, so XML depth is not applicable here. ~keep
        budget.depth = DepthValidator::new(limits.max_nesting_depth);
        budget
    }

    /// Convenience: build from `ExtractionConfig.security_limits` falling back to defaults.
    pub(crate) fn from_config(config: &crate::core::config::ExtractionConfig) -> Self {
        let owned: SecurityLimits;
        let limits: &SecurityLimits = match config.security_limits.as_ref() {
            Some(l) => l,
            None => {
                owned = SecurityLimits::default();
                &owned
            }
        };
        Self::from_limits(limits)
    }

    /// Build with explicit defaults (no config available, e.g. internal call sites).
    // `office` is here for the PPTX OMML sub-parse (#47): the roxmltree-based slide parser
    // threads no budget of its own, so the nested quick-xml math reader has nothing to
    // inherit and falls back to the default limits. ~keep
    #[cfg(any(feature = "xml", feature = "office"))]
    pub(crate) fn with_defaults() -> Self {
        Self::from_limits(&SecurityLimits::default())
    }

    /// Apply the iteration cap. Call once per parser-loop turn before reading an event.
    pub(crate) fn step(&mut self) -> Result<(), SecurityError> {
        self.iteration.check_iteration()
    }

    /// Apply nesting on a Start event. Call this after `step()` when the parser
    /// reaches an opening element / object / array / table / etc.
    pub(crate) fn enter(&mut self) -> Result<(), SecurityError> {
        self.depth.push()
    }

    /// Apply nesting on an End event. Saturates at zero on unbalanced input.
    pub(crate) fn leave(&mut self) {
        self.depth.pop();
    }

    /// Account for `len` bytes of emitted text. Returns `Err(ContentTooLarge)`
    /// once total output exceeds `max_content_size`.
    pub(crate) fn account_text(&mut self, len: usize) -> Result<(), SecurityError> {
        self.growth.check_append(len)
    }

    /// Validate an XML / HTML attribute value against `max_entity_length`.
    #[cfg(any(feature = "xml", feature = "office"))]
    pub(crate) fn check_attr(&self, name: &str, value: &str) -> Result<(), SecurityError> {
        self.entity.check_attr(name, value)
    }

    /// Validate a single entity / token string against `max_entity_length`.
    pub(crate) fn check_entity(&self, value: &str) -> Result<(), SecurityError> {
        self.entity.validate(value)
    }

    /// Account for `count` more table cells. Returns `Err(TooManyCells)` once
    /// the running total of cells exceeds `max_table_cells`.
    pub(crate) fn add_cells(&mut self, count: usize) -> Result<(), SecurityError> {
        self.table.add_cells(count)
    }
}

/// Return `true` when `path_str` contains a path-traversal component (`..`).
///
/// Uses [`std::path::Path::components`] rather than a string search so that
/// normalised representations (e.g. `a/../b`) are caught while benign values
/// like `"1..2"` in list-numbering prefixes are not falsely flagged.
///
/// # Examples
///
/// Not run as a doctest: this predicate is `pub(crate)`, used by the archive and
/// container extractors. The public entry point for archive safety is
/// [`crate::SecurityLimits`].
///
/// ```ignore
/// # use xberg::extractors::security::has_path_traversal;
/// assert!(has_path_traversal("word/../../etc/passwd"));
/// assert!(!has_path_traversal("word/images/photo.png"));
/// ```
#[allow(dead_code)]
pub(crate) fn has_path_traversal(path_str: &str) -> bool {
    use std::path::{Component, Path};
    Path::new(path_str).components().any(|c| c == Component::ParentDir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_limits() {
        let limits = SecurityLimits::default();
        assert_eq!(limits.max_archive_size, 500 * 1024 * 1024);
        assert_eq!(limits.max_nesting_depth, 1024);
        assert_eq!(limits.max_entity_length, 1024 * 1024);
    }

    #[test]
    fn test_string_growth_validator_basic() {
        let mut v = StringGrowthValidator::new(100);
        assert!(v.check_append(50).is_ok());
        assert_eq!(v.current_size, 50);
        assert!(v.check_append(50).is_ok());
        assert_eq!(v.current_size, 100);
        assert!(matches!(
            v.check_append(1),
            Err(SecurityError::ContentTooLarge { size: 101, max: 100 })
        ));
    }

    #[test]
    fn test_string_growth_validator_saturates_on_overflow() {
        let mut v = StringGrowthValidator::new(usize::MAX - 10);
        assert!(v.check_append(usize::MAX).is_err(), "saturating add cannot wrap");
    }

    #[test]
    fn test_iteration_validator_basic() {
        let mut v = IterationValidator::new(3);
        assert!(v.check_iteration().is_ok());
        assert!(v.check_iteration().is_ok());
        assert!(v.check_iteration().is_ok());
        assert!(matches!(
            v.check_iteration(),
            Err(SecurityError::TooManyIterations { count: 4, max: 3 })
        ));
    }

    #[test]
    fn test_depth_validator_push_pop() {
        let mut v = DepthValidator::new(3);
        assert!(v.push().is_ok());
        assert!(v.push().is_ok());
        assert!(v.push().is_ok());
        assert_eq!(v.current_depth, 3);
        assert!(matches!(
            v.push(),
            Err(SecurityError::NestingTooDeep { depth: 4, max: 3 })
        ));
        v.pop();
        assert_eq!(v.current_depth, 3);
    }

    #[test]
    fn test_depth_validator_pop_saturates_at_zero() {
        let mut v = DepthValidator::new(10);
        v.pop();
        v.pop();
        assert_eq!(v.current_depth, 0, "underflow is impossible");
    }

    #[test]
    fn test_entity_validator() {
        let v = EntityValidator::new(10);
        assert!(v.validate("short").is_ok());
        assert!(v.validate("0123456789").is_ok());
        assert!(matches!(
            v.validate("01234567890"),
            Err(SecurityError::EntityTooLong { length: 11, max: 10 })
        ));
        #[cfg(any(feature = "xml", feature = "office"))]
        {
            assert!(v.check_attr("href", "http://x").is_ok());
            assert!(v.check_attr("data", &"x".repeat(50)).is_err());
        }
    }

    #[test]
    fn test_table_validator() {
        let mut v = TableValidator::new(10);
        assert!(v.add_cells(5).is_ok());
        assert_eq!(v.current_cells, 5);
        assert!(v.add_cells(5).is_ok());
        assert_eq!(v.current_cells, 10);
        assert!(matches!(
            v.add_cells(1),
            Err(SecurityError::TooManyCells { cells: 11, max: 10 })
        ));
    }

    #[test]
    fn test_security_budget_depth_uses_the_tighter_of_the_two_configured_limits() {
        let nesting_is_tighter = SecurityLimits {
            max_xml_depth: 1024,
            max_nesting_depth: 5,
            ..SecurityLimits::default()
        };
        assert_eq!(
            SecurityBudget::from_limits(&nesting_is_tighter).depth.max_depth,
            5,
            "a tightened max_nesting_depth must not be discarded in favour of max_xml_depth"
        );

        let xml_is_tighter = SecurityLimits {
            max_xml_depth: 3,
            max_nesting_depth: 1024,
            ..SecurityLimits::default()
        };
        assert_eq!(
            SecurityBudget::from_limits(&xml_is_tighter).depth.max_depth,
            3,
            "a tightened max_xml_depth must not be discarded in favour of max_nesting_depth"
        );

        assert_eq!(
            SecurityBudget::from_limits(&SecurityLimits::default()).depth.max_depth,
            1024,
            "both defaults are 1024, so the default budget is unchanged"
        );
    }

    #[test]
    fn test_path_traversal_detected_in_simple_dotdot() {
        assert!(has_path_traversal("../etc/passwd"));
    }

    #[test]
    fn test_path_traversal_detected_in_middle_of_path() {
        assert!(has_path_traversal("word/images/../../etc/passwd"));
    }

    #[test]
    fn test_path_traversal_detected_at_end() {
        assert!(has_path_traversal("word/images/.."));
    }

    #[test]
    fn test_normal_path_not_flagged() {
        assert!(!has_path_traversal("word/images/photo.png"));
    }

    #[test]
    fn test_empty_path_not_flagged() {
        assert!(!has_path_traversal(""));
    }

    #[test]
    fn test_dotdot_in_filename_not_flagged() {
        assert!(!has_path_traversal("images/1..2.png"));
    }

    #[test]
    fn test_absolute_path_without_traversal_not_flagged() {
        assert!(!has_path_traversal("/usr/local/share/doc.pdf"));
    }
}
