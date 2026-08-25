//! Security utilities for document extractors.
//!
//! This module provides validation and protection mechanisms against common attacks:
//! - ZIP bomb detection (decompression bombs)
//! - XML entity expansion limits
//! - Nesting depth limits
//! - Input size limits
//! - Entity length validation
//! - Path traversal detection

#[cfg(any(
    feature = "archives",
    feature = "hwpx",
    feature = "iwork",
    feature = "office",
    feature = "excel"
))]
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

    /// Maximum nesting depth for structures (1024)
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

    /// Maximum XML depth (1024 levels)
    pub max_xml_depth: usize,

    /// Maximum cells per table (100,000)
    pub max_table_cells: usize,

    /// Maximum number of pages (or slides, or frames) in a single document.
    /// `None` means unlimited.
    ///
    /// Checked once the count is known and before any per-page work (OCR, layout
    /// detection, rendering) starts. Byte-size limits do not bound page count: a
    /// scanned page can compress to a few kilobytes, so a document well under
    /// `max_content_size` or `max_archive_size` can still hold thousands of pages
    /// of per-page work. Defaults to `None` (unlimited) because a real ceiling
    /// here is workload-specific and a low default would silently reject
    /// legitimate large documents; callers that want a ceiling set this
    /// explicitly.
    ///
    /// Enforced for: PDF (`extractors::pdf`, page count via `xberg_native_pdf`/`lopdf`),
    /// PPTX (`extraction::pptx`, slide count from the archive's slide parts),
    /// Keynote (`extractors::iwork::keynote`, slide count from `Index/Slide-*.iwa`
    /// entry names), ODP (`extractors::odp`, `draw:page` count in `content.xml`),
    /// and multi-frame TIFF images built with the `ocr` feature
    /// (`extractors::image`, frame count via the `tiff` crate). Not enforced for
    /// any other format, including DOCX, ODT, XLSX, legacy PPT/DOC, Pages/Numbers,
    /// and TIFF images when the `ocr` feature is disabled: those formats either
    /// have no fixed "page" the crate can count without doing the expensive work
    /// itself (DOCX/ODT page count is a layout outcome, not a stored value), or
    /// have no per-page pipeline to gate at all. Setting `max_pages` on a
    /// document of an unenforced format is silently a no-op, not a guarantee.
    // GH#764: modelled as `Option<usize>` rather than a `usize::MAX` sentinel, which had no
    // faithful representation in a generated binding -- alef reads `Default` impls into
    // concrete values, and a path expression it cannot fold yields the target language's zero,
    // which would have inverted "no page cap" into "reject every document" in all 15 bindings.
    // `Option<usize>` maps cleanly to None/nil/null/undefined everywhere, so the field now
    // generates instead of being skipped.
    pub max_pages: Option<usize>,
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
            max_pages: None,
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

    /// Document has too many pages
    TooManyPages {
        /// Number of pages found in the document.
        count: usize,
        /// Configured maximum page count.
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
            SecurityError::TooManyPages { count, max } => {
                write!(
                    f,
                    "Document has too many pages: {} (max: {}). Raise `security_limits.max_pages` \
                     if this document is legitimate, or split it before extraction.",
                    count, max
                )
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

/// Reject a document whose page count exceeds `max_pages`.
///
/// GH#1451. Every paginated format that can count cheaply before per-page work begins calls
/// this: PDF pages, PPTX/ODP/Keynote slides, multi-frame TIFF. Counting is what differs
/// between them; the comparison is not, and it had been copied verbatim into four modules.
///
/// `None` means unlimited, so an unset limit costs one branch and never rejects. The
/// comparison is `>` rather than `>=` deliberately -- a document exactly at the ceiling is
/// within it.
// Callers are the five paginated-format extractors, each behind its own feature: odp.rs and
// extraction/pptx/mod.rs (`office`), pdf/mod.rs (`pdf`), iwork/keynote.rs (`iwork`), and
// image.rs (the OCR trio, for multi-frame TIFF). A default build enables none of them, so this
// is gated to exactly that union rather than carrying `#[allow(dead_code)]`. No `test` arm:
// nothing tests it directly, and adding one would re-hide it.
#[cfg(any(
    feature = "office",
    feature = "pdf",
    feature = "iwork",
    feature = "ocr",
    feature = "ocr-wasm",
    feature = "ocr-pipeline"
))]
pub(crate) fn enforce_page_count(count: usize, max_pages: Option<usize>) -> Result<(), SecurityError> {
    match max_pages {
        Some(max) if count > max => Err(SecurityError::TooManyPages { count, max }),
        _ => Ok(()),
    }
}

/// Helper struct for validating ZIP archives for security issues.
#[cfg(any(
    feature = "archives",
    feature = "hwpx",
    feature = "iwork",
    feature = "office",
    feature = "excel"
))]
#[cfg_attr(alef, alef(skip))]
pub struct ZipBombValidator {
    limits: SecurityLimits,
}

#[cfg(any(
    feature = "archives",
    feature = "hwpx",
    feature = "iwork",
    feature = "office",
    feature = "excel"
))]
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

    /// The element depth at which [`SecurityBudget::enter`] starts to fail.
    #[cfg(feature = "office")]
    pub(crate) fn depth_limit(&self) -> usize {
        self.depth.max_depth
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

/// Error returned by [`resolve_container_entry`] when a container-relative entry name
/// cannot be safely resolved.
///
/// Deliberately narrow: this is about resolving a name against an archive-relative base
/// directory, not filesystem confinement. See [`crate::core::path_resolver`] for the
/// (unrelated) problem of confining a real filesystem read to a base directory.
#[cfg(any(feature = "office", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PathTraversalError {
    /// A `..` component popped past the container root: there was nothing left to remove.
    EscapesRoot,
    /// The target contains a NUL byte, which cannot appear in a legitimate archive entry name.
    InvalidByte,
    /// The target carries a Windows drive letter (`C:`) or UNC (`//server/share`) prefix.
    /// This function resolves names *inside* an archive, never a host filesystem path, so
    /// either form is rejected outright rather than treated as a literal path segment.
    DriveOrUncPrefix,
    /// Resolution produced no path segments at all (e.g. a bare `..` against a one-level
    /// base, or an input made up only of `.`/empty components).
    EmptyResult,
}

#[cfg(any(feature = "office", test))]
impl std::fmt::Display for PathTraversalError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::EscapesRoot => write!(f, "path escapes the container root"),
            Self::InvalidByte => write!(f, "path contains a NUL byte"),
            Self::DriveOrUncPrefix => write!(f, "path carries a drive letter or UNC prefix"),
            Self::EmptyResult => write!(f, "path resolves to no entry"),
        }
    }
}

#[cfg(any(feature = "office", test))]
impl std::error::Error for PathTraversalError {}

/// Resolve a container-relative entry name against a base directory inside a ZIP-based
/// container (an OOXML part, an EPUB package, ...).
///
/// This is **boundary-relative**, not a `..`-blacklist: an in-bounds `..` that leaves and
/// returns without crossing the container root is allowed, because that is the normal,
/// spec-correct form of many OPC/EPUB relationships (`../media/image1.png` is exactly how a
/// PPTX slide references an image one directory up, and how a DOCX `word/_rels/document.xml.rels`
/// entry references an image at the package root's `media/`). Only a `..` that would pop
/// past the root is rejected. This replaces the deleted `has_path_traversal`, which rejected
/// every `..` unconditionally and would have broken every one of those legitimate references.
///
/// `base` is the container-relative directory the reference resolves against (e.g. `"word"`,
/// `"ppt/slides"`, `"OEBPS/text"`; `""` or `"."` means the container root). A leading `/` in
/// `target` means "relative to the container root" per the OPC/EPUB convention -- not the
/// host filesystem -- and overrides `base` entirely.
///
/// Backslashes in `target` are normalised to `/` explicitly rather than relying on
/// [`std::path`], whose component parsing is target-OS-dependent: the same source can treat
/// `a\..\..\x` as one opaque literal on Unix and as three components on Windows. A drive
/// letter (`C:`) or UNC prefix (`//server/share`, from a normalised `\\server\share`) is
/// rejected outright. `base` is not backslash-normalised: every real caller builds it from
/// `/`-delimited container-relative names (a hardcoded literal, or a directory sliced out of
/// an entry name that itself uses `/`), never from raw attacker input.
///
/// Percent-decoding is deliberately **not** performed here; it is format-specific (an EPUB
/// href is a URL, an OOXML `Target` attribute is not). A caller that needs it must decode
/// *before* calling this function -- decoding after would let a decoded `../` slip past a
/// boundary check that already ran.
// Every real caller (DOCX, EPUB, PPTX) lives behind `#[cfg(feature = "office")]`, so this
// whole group compiles out with that feature off rather than carrying a blanket
// `#[allow(dead_code)]`, which would also mask a genuinely-unused item appearing later.
// `test` is OR'd in so the unit tests below still reach it under a non-office test build.
#[cfg(any(feature = "office", test))]
pub(crate) fn resolve_container_entry(base: &str, target: &str) -> Result<String, PathTraversalError> {
    if target.contains('\0') {
        return Err(PathTraversalError::InvalidByte);
    }

    let normalized_target = target.replace('\\', "/");
    if is_drive_or_unc_prefixed(&normalized_target) {
        return Err(PathTraversalError::DriveOrUncPrefix);
    }

    let mut stack: Vec<&str> = Vec::new();
    let effective: &str = match normalized_target.strip_prefix('/') {
        Some(root_relative) => root_relative,
        None => {
            for segment in base.split('/') {
                push_segment(&mut stack, segment)?;
            }
            normalized_target.as_str()
        }
    };

    for segment in effective.split('/') {
        push_segment(&mut stack, segment)?;
    }

    if stack.is_empty() {
        return Err(PathTraversalError::EmptyResult);
    }

    Ok(stack.join("/"))
}

/// Apply one `/`-delimited path segment to the working stack: push a normal component,
/// ignore `.` and empty components, and pop on `..` -- erroring if there is nothing left to
/// pop. Shared between the `base` and `target` halves of [`resolve_container_entry`] so the
/// pop-underflow rule is exactly one rule, applied identically on both sides of the join.
#[cfg(any(feature = "office", test))]
fn push_segment<'a>(stack: &mut Vec<&'a str>, segment: &'a str) -> Result<(), PathTraversalError> {
    match segment {
        "" | "." => {}
        ".." => {
            if stack.pop().is_none() {
                return Err(PathTraversalError::EscapesRoot);
            }
        }
        _ => stack.push(segment),
    }
    Ok(())
}

/// `true` when `path` begins with a Windows drive letter (`C:`) or a UNC prefix (`//`, which
/// is what `\\server\share` becomes after backslash normalisation).
#[cfg(any(feature = "office", test))]
fn is_drive_or_unc_prefixed(path: &str) -> bool {
    let bytes = path.as_bytes();
    path.starts_with("//") || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
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
    fn test_default_max_pages_is_unlimited() {
        // A default that rejects real documents would be worse than the risk it
        // mitigates (issue #1451): the ceiling only applies once a caller opts in.
        assert_eq!(SecurityLimits::default().max_pages, None);
    }

    #[test]
    fn test_too_many_pages_display_names_the_limit() {
        let error = SecurityError::TooManyPages {
            count: 4_000,
            max: 1_000,
        };
        let message = error.to_string();
        assert!(
            message.contains("4000"),
            "message must name the observed count: {message}"
        );
        assert!(
            message.contains("1000"),
            "message must name the configured max: {message}"
        );
        assert!(
            message.contains("max_pages"),
            "message must name the limit that was hit: {message}"
        );
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

    // `resolve_container_entry` matrix. Each case below is named after the input class
    // from the path-traversal unification brief so the test file doubles as the
    // executable version of that comparison table.

    #[test]
    fn parent_relative_target_in_bounds_pops_into_the_root() {
        // "../x" against a one-level base: the ".." exactly cancels "a", landing on "x"
        // at the container root. This is the PPTX/DOCX "spec-correct ../media/x.png" shape.
        assert_eq!(resolve_container_entry("a", "../x"), Ok("x".to_string()));
    }

    #[test]
    fn parent_relative_target_out_of_bounds_at_the_root_is_rejected() {
        // Same "../x", but there is no base directory left to pop: this is a real escape.
        assert_eq!(
            resolve_container_entry("", "../x"),
            Err(PathTraversalError::EscapesRoot)
        );
    }

    #[test]
    fn double_parent_within_a_single_level_base_escapes() {
        // "a/../../x": one push, then two pops. The base is empty, so after the local
        // "a" is popped there is nothing left for the second "..".
        assert_eq!(
            resolve_container_entry("", "a/../../x"),
            Err(PathTraversalError::EscapesRoot)
        );
    }

    #[test]
    fn double_parent_within_a_two_level_base_is_in_bounds() {
        // Same shape, but the base has enough depth ("root") to absorb both "a" and the
        // outer "..": the whole path collapses to a container-root-relative "x".
        assert_eq!(resolve_container_entry("root", "a/../../x"), Ok("x".to_string()));
    }

    #[test]
    fn absolute_target_is_root_relative_and_ignores_base() {
        // A leading "/" means "relative to the container root" (the OPC/EPUB convention),
        // not the host filesystem -- `base` is completely bypassed.
        assert_eq!(resolve_container_entry("word", "/abs/x"), Ok("abs/x".to_string()));
    }

    #[test]
    fn windows_drive_letter_target_is_rejected() {
        assert_eq!(
            resolve_container_entry("word", "C:\\x"),
            Err(PathTraversalError::DriveOrUncPrefix)
        );
    }

    #[test]
    fn unc_style_target_is_rejected() {
        // "\\server\share\x" normalises to "//server/share/x", which is caught by the
        // same UNC check as a literal "//..." input -- no separate UNC-specific parsing.
        assert_eq!(
            resolve_container_entry("word", "\\\\server\\share\\x"),
            Err(PathTraversalError::DriveOrUncPrefix)
        );
    }

    #[test]
    fn backslash_traversal_is_normalised_the_same_as_forward_slash() {
        // "a\..\..\x": backslashes are converted to "/" explicitly, so this behaves
        // identically on every build platform instead of depending on `std::path`'s
        // target-dependent component parsing (the drift `has_path_traversal` had).
        assert_eq!(
            resolve_container_entry("", "a\\..\\..\\x"),
            Err(PathTraversalError::EscapesRoot)
        );
    }

    #[test]
    fn dot_segments_are_transparent_to_in_bounds_traversal() {
        // "a/./../x": the "." is a no-op and the ".." cancels "a", leaving "x".
        assert_eq!(resolve_container_entry("", "a/./../x"), Ok("x".to_string()));
    }

    #[test]
    fn four_dots_is_a_literal_component_not_a_traversal_token() {
        // "....//x": "...." is not the exact string "..", so it is pushed as an ordinary
        // (if unusual) literal segment. The doubled slash contributes an empty component,
        // which is dropped.
        assert_eq!(resolve_container_entry("", "....//x"), Ok("..../x".to_string()));
    }

    #[test]
    fn bare_dotdot_against_a_one_level_base_has_no_file_left_to_resolve() {
        assert_eq!(resolve_container_entry("a", ".."), Err(PathTraversalError::EmptyResult));
    }

    #[test]
    fn bare_dotdot_against_the_root_escapes() {
        assert_eq!(resolve_container_entry("", ".."), Err(PathTraversalError::EscapesRoot));
    }

    #[test]
    fn empty_components_are_skipped() {
        assert_eq!(resolve_container_entry("", "a//b"), Ok("a/b".to_string()));
    }

    #[test]
    fn trailing_dotdot_resolves_to_the_base_directory_itself() {
        // "a/..": pushes "a" onto "root" then immediately pops it back off, landing
        // exactly on the base -- allowed, even though the result names a directory
        // rather than a file (the caller's `by_name` lookup will simply miss).
        assert_eq!(resolve_container_entry("root", "a/.."), Ok("root".to_string()));
    }

    #[test]
    fn nul_byte_is_rejected_outright() {
        assert_eq!(
            resolve_container_entry("word", "media/\0image1.png"),
            Err(PathTraversalError::InvalidByte)
        );
    }

    #[test]
    fn percent_encoded_traversal_is_never_decoded_by_this_function() {
        // "%2e%2e%2f" contains no literal '/' -- it is one opaque literal segment here.
        // Decoding is the caller's job, and must happen *before* calling this function
        // (EPUB's `resolve_path` does exactly that); decoding afterwards would let a
        // decoded "../" slip past a boundary check that already ran.
        assert_eq!(
            resolve_container_entry("base", "%2e%2e%2f"),
            Ok("base/%2e%2e%2f".to_string())
        );
    }

    #[test]
    fn multibyte_character_after_dotdot_is_a_literal_segment_not_a_slice_panic() {
        // ".." followed by a 4-byte emoji is not the exact string "..", so the whole
        // thing is pushed as a literal segment. Exact string comparison (rather than the
        // old PPTX code's fixed-byte-offset slice) can never land mid-character.
        assert_eq!(
            resolve_container_entry("base", "..\u{1F600}/x"),
            Ok("base/..\u{1F600}/x".to_string())
        );
    }

    // DOCX-specific cases: `docx.rs` calls this with `base = "word"`. These pin the two
    // behaviours called out in the unification plan as a deliberate change from the
    // deleted `has_path_traversal`, which rejected every `..` unconditionally.

    #[test]
    fn docx_word_relative_target_climbs_to_the_package_root_media_directory() {
        // The normal shape for a DOCX image whose relationship lives at the package
        // root's "media/" directory, one level above "word/". `has_path_traversal` used
        // to reject this outright; it is legitimate and must resolve.
        assert_eq!(
            resolve_container_entry("word", "../media/image1.png"),
            Ok("media/image1.png".to_string())
        );
    }

    #[test]
    fn docx_word_relative_target_that_truly_escapes_the_package_still_errors() {
        assert_eq!(
            resolve_container_entry("word", "../../../etc/passwd"),
            Err(PathTraversalError::EscapesRoot)
        );
    }

    #[test]
    fn docx_absolute_target_reroots_to_the_package_relative_name() {
        // `has_path_traversal` allowed this (asserted deliberately at what was
        // `security.rs:891`) and DOCX then re-rooted it by hand with `strip_prefix('/')`.
        // The shared helper folds that re-rooting into the same call.
        assert_eq!(
            resolve_container_entry("word", "/media/image1.png"),
            Ok("media/image1.png".to_string())
        );
    }
}
