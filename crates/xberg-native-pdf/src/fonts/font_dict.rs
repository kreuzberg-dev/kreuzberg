//! Font dictionary parsing.
//!
//! This module handles parsing of PDF font dictionaries and encoding information.
//! Fonts in PDF can have various encodings, and the ToUnicode CMap provides the
//! most accurate character-to-Unicode mapping.

use super::adobe_glyph_list::ADOBE_GLYPH_LIST;
use crate::document::PdfDocument;
use crate::error::{Error, Result};
use crate::fonts::TrueTypeCMap;
use crate::fonts::cmap::LazyCMap;
use crate::layout::text_block::FontWeight;
use crate::object::Object;
use skrifa::instance::{LocationRef, Size};
use skrifa::metrics::{GlyphMetrics, Metrics};
use skrifa::{FontRef, GlyphId, MetadataProvider};
use std::collections::HashMap;
use std::sync::Arc;

/// Name-derived Standard-14 classification of a font, resolved once and
/// memoized (see [`FontInfo::std14_memo`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Std14Flags {
    /// Font is one of the Times family.
    pub is_times: bool,
    /// Font is one of the Courier (monospace) family.
    pub is_courier: bool,
    /// Font name carries a Bold marker.
    pub is_bold: bool,
    /// Font name carries a BoldItalic marker.
    pub is_bold_italic: bool,
    /// Font is one of the Helvetica family.
    pub is_helvetica: bool,
    /// Font name carries an Italic marker.
    pub is_italic: bool,
}

/// Font information extracted from a PDF font dictionary.
#[derive(Debug, Clone)]
pub struct FontInfo {
    /// Base font name (e.g., "Times-Roman", "Helvetica-Bold")
    pub base_font: String,
    /// Font subtype (e.g., "Type1", "TrueType", "Type0")
    pub subtype: String,
    /// Encoding information
    pub encoding: Encoding,
    /// ToUnicode CMap (character code to Unicode mapping)
    /// Lazily parsed on first character lookup for improved performance
    pub to_unicode: Option<LazyCMap>,
    /// Font weight from FontDescriptor (400 = normal, 700 = bold)
    pub font_weight: Option<i32>,
    /// Font descriptor flags (bit field)
    /// Bit 1: FixedPitch, Bit 2: Serif, Bit 3: Symbolic, Bit 4: Script,
    /// Bit 6: Nonsymbolic, Bit 7: Italic
    /// PDF Spec: ISO 32000-1:2008, Table 5.20
    pub flags: Option<i32>,
    /// Stem thickness (vertical) from FontDescriptor (used for weight inference)
    /// PDF Spec: ISO 32000-1:2008, Section 9.6.2
    /// Typical values: <80 = light, 80-110 = normal/medium, >110 = bold
    pub stem_v: Option<f32>,
    /// Ascent above the baseline (fraction of em, from FontDescriptor /Ascent).
    /// Converted from PDF's 1/1000-em units to a fraction of em (raw value ÷ 1000).
    /// Defaults to 0.95 when the font descriptor is absent (matching Poppler's fallback).
    pub ascent: f32,
    /// Descent below the baseline (fraction of em, from FontDescriptor /Descent).
    /// Converted from PDF's 1/1000-em units to a fraction of em; always ≤ 0.
    /// Defaults to -0.35 when the font descriptor is absent (matching Poppler's fallback).
    pub descent: f32,
    /// Embedded TrueType font data (from FontFile2 stream)
    /// Shared via Arc to avoid expensive cloning
    pub embedded_font_data: Option<Arc<Vec<u8>>>,
    /// Lazily-extracted TrueType cmap table (GID to Unicode mappings).
    /// Used as fallback when ToUnicode CMap is missing.
    /// Initialized on first access via `truetype_cmap()` accessor to avoid
    /// the 10-25ms per-font extraction cost when ToUnicode resolves all chars.
    pub truetype_cmap: std::sync::OnceLock<Option<TrueTypeCMap>>,
    /// Lazily-extracted embedded TrueType/CFF `post`-table glyph names,
    /// indexed by GID. `None` element = no name for that GID (post format 3,
    /// or the glyph name table is absent). Used by §9.10.2 Priority 3c
    /// fallback in `decode_char_to_unicode`: when `truetype_cmap.get_unicode`
    /// misses, we try this glyph name via `glyph_name_to_unicode` (AGL +
    /// `uniXXXX`/`uXXXXX` synth) before falling through to the hardcoded
    /// `gid_to_standard_glyph_name` ASCII map and CID-as-Unicode last
    /// resort. Resolves `•` → `❍` substitution and `fi`/`fl` ligature
    /// corruption on Identity-H subset fonts without `CIDToGIDMap`.
    ///
    pub embedded_glyph_names: std::sync::OnceLock<Option<Vec<Option<String>>>>,
    /// Whether this font has an embedded TrueType font (FontFile2).
    /// Controls whether lazy truetype_cmap extraction is attempted.
    pub is_truetype_font: bool,
    /// CID to GID mapping (Type0 fonts only, Phase 3)
    /// Converts Character IDs in the PDF to Glyph IDs in the embedded font
    /// Used to look up Unicode values via the TrueType cmap table
    /// Phase 3: Enables CFF/OpenType support via CIDToGIDMap parsing
    pub cid_to_gid_map: Option<CIDToGIDMap>,
    /// CIDFont character collection info (Type0 fonts only)
    /// Identifies the character set (e.g., Adobe-Japan1, Adobe-GB1)
    pub cid_system_info: Option<CIDSystemInfo>,
    /// CIDFont subtype ("CIDFontType0" for CFF, "CIDFontType2" for TrueType)
    pub cid_font_type: Option<String>,
    /// `FontMatrix[a]` element — scales glyph-space widths to text-space units.
    /// Standard Type1/TrueType: 0.001 (widths in 1/1000 em).
    /// Type3 with `FontMatrix [1 0 0 1 0 0]`: 1.0 (widths already in text-space units).
    /// `advance_in_text_space = width × font_matrix_a × font_size`
    pub font_matrix_a: f32,
    /// Character widths in 1000ths of em (PDF units)
    /// For simple fonts (Type1, TrueType): array indexed by (char_code - first_char)
    /// PDF Spec: ISO 32000-1:2008, Section 9.7.4
    pub widths: Option<Vec<f32>>,
    /// First character code covered by widths array
    /// Used to map character codes to width array indices
    pub first_char: Option<u32>,
    /// Last character code covered by widths array
    pub last_char: Option<u32>,
    /// Default width for characters not in widths array (in 1000ths of em)
    /// Typical values: 500-600 for proportional fonts, 600 for monospace
    pub default_width: f32,
    /// CID to width mapping for Type0 (CIDFont) fonts
    /// Per PDF Spec ISO 32000-1:2008, Section 9.7.4.3
    /// Widths in 1000ths of em. Uses HashMap for sparse CID distributions.
    pub cid_widths: Option<HashMap<u16, f32>>,
    /// Default width for CIDs not in cid_widths (Type0 fonts only)
    /// Per PDF Spec: default is 1000 if /DW not specified
    pub cid_default_width: f32,
    /// Whether /DW was explicitly present in the CIDFont dictionary.
    /// Used by has_explicit_widths() and get_glyph_width() to distinguish
    /// a spec-default 1000 from an authored 1000 (F14/F15 fix).
    pub has_explicit_dw: bool,
    /// Multi-character encoding map for compound glyph names (e.g. f_f → "ff")
    /// Stores mappings from character code to multi-char strings
    pub multi_char_map: HashMap<u8, String>,
    /// CFF byte_code → glyph_id mapping for embedded CFF subset fonts.
    /// Allows direct glyph rendering without Unicode cmap.
    pub cff_gid_map: Option<HashMap<u8, u16>>,
    /// Pre-computed byte→char lookup for simple (non-Type0) fonts.
    /// Index by byte value (0-255). '\0' means "use full char_to_unicode fallback".
    /// Built lazily on first text decode. Avoids per-byte HashMap lookups.
    pub byte_to_char_table: std::sync::OnceLock<[char; 256]>,
    /// Per-font memo of `char_to_unicode`. Type0/CID fonts have no
    /// `byte_to_char_table`, so without this each glyph re-runs the decode
    /// cascade. `Arc<Mutex<…>>` keeps `FontInfo: Clone` (clones share the memo).
    pub type0_unicode_memo: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<u32, Option<String>>>>,
    /// Pre-computed byte→width lookup for simple (non-Type0) fonts.
    /// Index by byte value (0-255). Built lazily on first advance_position call.
    /// Eliminates per-byte bounds check and subtraction in get_glyph_width.
    pub byte_to_width_table: std::sync::OnceLock<[f32; 256]>,
    /// Memo of [`FontInfo::get_font_weight`]. The name-based fallback lowercases
    /// `base_font` and runs a dozen substring searches; text extraction asks for
    /// the weight once per glyph, where the answer is loop-invariant.
    pub weight_memo: std::sync::OnceLock<FontWeight>,
    /// Memo of [`FontInfo::is_italic`] — same per-glyph hot path as `weight_memo`.
    pub italic_memo: std::sync::OnceLock<bool>,
    /// Memo of the Standard-14 name classification. `get_standard_font_width`
    /// is called per glyph and otherwise re-strips the subset prefix and
    /// re-scans a 15-name table every time.
    pub std14_memo: std::sync::OnceLock<Option<Std14Flags>>,
    /// Raw `/Differences` glyph names retained by character code (simple fonts).
    /// Populated alongside the `Encoding::Custom` map during `parse_encoding`,
    /// but unlike the Custom map (which stores the *resolved* char) this keeps the
    /// authoritative glyph *name* the writer assigned via the encoding dictionary's
    /// `/Differences` array (ISO 32000-1 §9.6.6.1, Table 114). Used by
    /// `glyph_name_for_code` to recover punctuation (`period`/`comma`/`hyphen`/
    /// `minus`) when an upstream decode yields a non-sensible symbol — see the
    /// glyph-name-gated interceptions in `char_to_unicode`.
    pub diff_glyph_names: HashMap<u8, String>,
    /// Writing mode resolved from this font's encoding and (when available)
    /// from the embedded CMap stream's `/WMode` directive.
    ///
    /// - `0` (default): horizontal writing — glyph advance along x-axis.
    /// - `1`: vertical writing (tategaki) — glyph advance along y-axis with
    ///   per-CID vertical-origin offset applied per glyph.
    ///
    /// Resolution rules (highest precedence first):
    /// 1. The embedded CMap stream's `/WMode` directive when one is parsed
    ///    (via `LazyCMap::wmode()` on the encoding's CMap).
    /// 2. Predefined PDF CMap name ending in `-V` (Identity-V, UniJIS-UTF16-V,
    ///    UniGB-UTF16-V, UniCNS-UTF16-V, UniKS-UTF16-V) or the bare legacy
    ///    `V`. The original encoding name is retained even when the
    ///    `Encoding` enum collapses `Identity-H`/`Identity-V` into
    ///    `Encoding::Identity`.
    /// 3. Otherwise `0`.
    pub wmode: u8,
    /// Per-CID vertical-writing metrics parsed from the CIDFont's `/W2`
    /// array (ISO 32000-1 §9.7.4.3). `None` for horizontal-only fonts so
    /// they pay no allocation/hash-lookup cost.
    pub cid_vertical_metrics: Option<HashMap<u16, VerticalMetrics>>,
    /// Default vertical metrics for CIDs not covered by `cid_vertical_metrics`.
    /// Parsed from `/DW2` (defaults to [`VerticalMetrics::SPEC_DEFAULT`] when
    /// `/DW2` is absent). Held by value because the struct is `Copy`.
    pub cid_default_vertical_metrics: VerticalMetrics,
    /// `Some(collection)` when this is a Type0 CIDFont referencing one of
    /// Adobe's predefined CJK base names (`Ryumin-Light`, `GothicBBB-Medium`,
    /// `STSong-Light`, `MHei-Medium`, `HYSMyeongJo-Medium`, …), has no
    /// embedded font program (no `/FontFile{,2,3}` key on either the Type0
    /// wrapper's or the CIDFont descendant's descriptor), AND uses an
    /// Identity charcode→CID `/Encoding` (Identity-H/V or an
    /// Adobe-collection identity CMap stream). ISO 32000-2 §9.7.5.2 requires
    /// a conforming reader to supply glyphs for these character collections;
    /// the renderer consults this field to route the paint through a bundled
    /// covering font (see [`super::predefined_cidfont`]) and convert each CID
    /// through the appropriate [`super::cid_mappings`] table to a Unicode
    /// code point. The collection follows the descendant's `/CIDSystemInfo`
    /// Ordering when it names a known collection, falling back to the
    /// name-derived collection for Identity/unknown orderings.
    ///
    /// `None` for every other font, including:
    /// - Type0 fonts whose CIDFont declares an embedded program — even when
    ///   that program fails to load/decode, substitution would mask the
    ///   decode defect; the failure is logged instead;
    /// - Type0 fonts with a non-Identity predefined CMap (`90ms-RKSJ-H`,
    ///   `GBK-EUC-H`, …) whose charcodes are raw legacy multi-byte values,
    ///   not CIDs — unsupported until a charcode→CID CMap pass is wired;
    /// - Type0 fonts whose base name is not in the predefined registry (we
    ///   cannot safely guess a substitution);
    /// - Simple Type1 / TrueType fonts.
    pub cjk_substitution: Option<super::predefined_cidfont::CharacterCollection>,
}

/// Font encoding types.
#[derive(Debug, Clone)]
pub enum Encoding {
    /// Standard PDF encoding (WinAnsiEncoding, MacRomanEncoding, etc.)
    Standard(String),
    /// Custom encoding with explicit character mappings
    Custom(HashMap<u8, char>),
    /// Identity encoding (typically used for CID fonts)
    Identity,
}

/// CID to GID mapping for Type 2 CIDFonts (TrueType-based)
/// Per PDF Spec ISO 32000-1:2008, Section 9.7.4.2
///
/// This mapping converts Character IDs (CIDs) in the PDF document to Glyph IDs (GIDs)
/// in the embedded TrueType font, which can then be mapped to Unicode via the cmap table.
#[derive(Debug, Clone)]
pub enum CIDToGIDMap {
    /// Identity mapping: CID == GID (default, most common)
    /// Used when each character ID directly corresponds to a glyph ID
    Identity,

    /// Explicit mapping: CID → GID via uint16 stream
    /// Stream format: GID at bytes [2*CID, 2*CID+1], big-endian
    /// Used for non-standard glyph ID assignments
    Explicit(Vec<u16>),
}

impl CIDToGIDMap {
    /// Convert a Character ID (CID) to a Glyph ID (GID) using this mapping.
    ///
    /// Per PDF Spec ISO 32000-1:2008, Section 9.7.4.2:
    /// - Identity mapping: CID == GID (most common, default)
    /// - Explicit mapping: Use uint16 array lookup
    ///
    /// # Arguments
    ///
    /// * `cid` - The Character ID from the PDF document
    ///
    /// # Returns
    ///
    /// The corresponding Glyph ID in the embedded font
    pub fn get_gid(&self, cid: u16) -> u16 {
        match self {
            CIDToGIDMap::Identity => cid,
            CIDToGIDMap::Explicit(gid_array) => {
                if (cid as usize) < gid_array.len() {
                    gid_array[cid as usize]
                } else {
                    cid
                }
            }
        }
    }
}

/// CIDFont character collection identifier
/// Per PDF Spec ISO 32000-1:2008, Section 9.7.4.2
///
/// Identifies which character encoding the CIDFont uses, such as:
/// - Adobe-Japan1: Japanese text
/// - Adobe-GB1: Simplified Chinese
/// - Adobe-CNS1: Traditional Chinese
/// - Adobe-Korea1: Korean
#[derive(Debug, Clone)]
pub struct CIDSystemInfo {
    /// Registry name (typically "Adobe")
    pub registry: String,

    /// Ordering string (e.g., "Japan1", "GB1", "CNS1", "Korea1")
    pub ordering: String,

    /// Supplement number (version of the character collection)
    pub supplement: i32,
}

/// Per-CID vertical-writing metrics from a CIDFont's `/W2` array.
///
/// Per ISO 32000-1:2008 §9.7.4.3 and the Adobe CMap & CIDFont Files
/// Specification §9.7. In vertical writing mode the glyph advances along the
/// y-axis (not the x-axis) and is shifted from its default horizontal origin
/// to a vertical origin so that the glyph stacks correctly within a column.
///
/// All values are in 1000ths-of-em (glyph-space units), matching the
/// convention used throughout PDF font dictionaries.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VerticalMetrics {
    /// `w1y`: vertical displacement (advance) of the glyph along the y-axis.
    ///
    /// Typically negative (around `-1000` for a full-em CJK glyph) because PDF
    /// user space has y increasing upward, while vertical text advances
    /// downward. The text matrix is translated by `w1y * font_size / 1000`
    /// after the glyph is painted.
    pub w1y: f32,

    /// `v_x`: x-component of the vector from the default (horizontal) origin
    /// to the vertical origin, in 1000ths-of-em.
    ///
    /// Spec default `500` (half-em) places the vertical origin at the glyph's
    /// horizontal center, which is correct for monospaced full-width CJK
    /// glyphs.
    pub v_x: f32,

    /// `v_y`: y-component of the vertical-origin offset, in 1000ths-of-em.
    ///
    /// Spec default `880` places the vertical origin near the top of the em.
    pub v_y: f32,
}

impl VerticalMetrics {
    /// Spec default per ISO 32000-1 §9.7.4.3: vertical origin at
    /// `(500, 880)` and glyph displacement `-1000` (one full em downward).
    pub const SPEC_DEFAULT: VerticalMetrics = VerticalMetrics {
        w1y: -1000.0,
        v_x: 500.0,
        v_y: 880.0,
    };
}

/// Decide writing mode from a predefined PDF CMap name.
///
/// Per ISO 32000-1 §9.7.5.2 (Table 118) and the Adobe CMap & CIDFont Files
/// Specification, predefined CMap names whose suffix is `-V` (e.g.
/// `Identity-V`, `UniJIS-UTF16-V`, `UniGB-UTF16-V`, `UniCNS-UTF16-V`,
/// `UniKS-UTF16-V`, `GBK-EUC-V`, `90ms-RKSJ-V`, …) and the bare legacy `V`
/// declare vertical writing (`/WMode 1`). Every other name implies
/// horizontal writing (`/WMode 0`).
///
/// This function is the canonical name-to-wmode decision used by both
/// `FontInfo::resolve_encoding_writing_mode` and the encoding-name fallback
/// inside `FontInfo::from_dict`.
pub(crate) fn wmode_from_predefined_cmap_name(name: &str) -> u8 {
    if name == "V" || name.ends_with("-V") { 1 } else { 0 }
}

impl FontInfo {
    /// Get the TrueType cmap, lazily extracting it on first access.
    /// Returns `None` if the font is not TrueType or has no embedded data.
    pub fn truetype_cmap(&self) -> Option<&TrueTypeCMap> {
        self.truetype_cmap
            .get_or_init(|| {
                if !self.is_truetype_font {
                    return None;
                }
                let font_data = self.embedded_font_data.as_ref()?;
                if font_data.is_empty() {
                    return None;
                }
                match TrueTypeCMap::from_font_data(font_data) {
                    Ok(cmap) if !cmap.is_empty() => {
                        tracing::debug!(
                            "Lazy-extracted TrueType cmap for font '{}': {} mappings",
                            self.base_font,
                            cmap.len()
                        );
                        Some(cmap)
                    }
                    Ok(_) => None,
                    Err(_) => {
                        tracing::warn!(
                            target: crate::LOG_TARGET_ROOT,
                            operation = "extract_truetype_cmap",
                            error_code = "invalid_font_data",
                            "using fallback font encoding"
                        );
                        None
                    }
                }
            })
            .as_ref()
    }

    /// Set the TrueType cmap directly (used by share_truetype_cmaps and tests).
    pub fn set_truetype_cmap(&mut self, cmap: Option<TrueTypeCMap>) {
        self.truetype_cmap = std::sync::OnceLock::new();
        if let Some(c) = cmap {
            let _ = self.truetype_cmap.set(Some(c));
        } else {
            let _ = self.truetype_cmap.set(None);
        }
    }

    /// Check if a TrueType cmap is available (either already extracted or extractable).
    pub fn has_truetype_cmap(&self) -> bool {
        self.truetype_cmap().is_some()
    }

    /// The most authoritative Unicode-mapping resource this font offers, as a
    /// [`MappingProvenance`](crate::fonts::MappingProvenance).
    ///
    /// This is a **fact** derived from the font's structure — which mapping
    /// resources exist — not a decode of any particular character code. It
    /// mirrors the ISO 32000-1 §9.10.2 priority order and covers every font
    /// type, so it is complete where a font-type-specific structural check is
    /// not.
    ///
    /// [`Fallback`](crate::fonts::MappingProvenance::Fallback) is the important
    /// value: it means the font carries **no** mapping resource — no usable
    /// `/ToUnicode`, no predefined CID→Unicode collection, no embedded `cmap`,
    /// and no simple-font encoding — so any Unicode extracted for its glyphs is
    /// a fabricated echo, not read from the file (§9.10.2: "there is no way to
    /// determine what the character code represents"). Callers compose their own
    /// policy from this (route to OCR, flag the page, keep the raw echo).
    pub fn best_mapping_provenance(&self) -> crate::fonts::MappingProvenance {
        use crate::fonts::MappingProvenance as P;
        // 1. A present, non-empty /ToUnicode CMap is authoritative (§9.10.2). ~keep
        if self
            .to_unicode
            .as_ref()
            .and_then(|c| c.get())
            .is_some_and(|m| !m.is_empty())
        {
            return P::ToUnicode;
        }
        // 2. A predefined CID→Unicode collection: a Type0 font whose descendant
        //    uses a known, non-Identity ordering (Adobe-GB1/CNS1/Japan1/Korea1). ~keep
        if self.subtype == "Type0"
            && let Some(info) = &self.cid_system_info
            && info.ordering != "Identity"
            && !info.ordering.is_empty()
        {
            return P::PredefinedCMap;
        }
        // 3. The embedded program's own cmap (recoverable byte-as-GID / Identity
        //    subsets that kept a usable cmap). ~keep
        if self.has_truetype_cmap() {
            return P::EmbeddedCmap;
        }
        // 4. A simple font resolves through its /Encoding → glyph name → AGL, and
        //    symbolic Symbol/ZapfDingbats through their built-in encodings. ~keep
        if self.subtype != "Type0" {
            return P::EncodingName;
        }
        // 5. A Type0 font with none of the above severs every path to Unicode. ~keep
        P::Fallback
    }

    /// Look up the embedded font program's `post`-table glyph name for the
    /// given GID.
    ///
    /// Lazily parses the embedded TrueType/OpenType font (via `ttf-parser`)
    /// on first access, then caches a `Vec<Option<String>>` indexed by GID
    /// for O(1) subsequent lookups. The parsed font's `Face::glyph_name`
    /// abstracts over TrueType `post` Format 2 names and CFF `charset` SIDs,
    /// so this works for both TrueType (FontFile2) and CFF / Type1C
    /// (FontFile3) subset fonts.
    ///
    /// Returns `None` when:
    /// - the font has no embedded program (`embedded_font_data == None`),
    /// - the font program is empty or fails to parse,
    /// - the `post` table is Format 3 (no names) or the GID is out of range,
    /// - the parsed name is `.notdef` (which AGL doesn't map and isn't
    ///   useful as text anyway).
    ///
    /// Used by §9.10.2 Priority 3c in `decode_char_to_unicode`.
    pub(crate) fn embedded_glyph_name(&self, gid: u16) -> Option<&str> {
        let names = self
            .embedded_glyph_names
            .get_or_init(|| {
                let font_data = self.embedded_font_data.as_ref()?;
                if font_data.is_empty() {
                    return None;
                }
                let font = match skrifa::raw::FontRef::new(font_data) {
                    Ok(f) => f,
                    Err(e) => {
                        tracing::debug!(
                            "Font '{}': FontRef::new failed for glyph-name extraction: {:?}",
                            self.base_font,
                            e
                        );
                        return None;
                    }
                };
                let glyph_names = skrifa::MetadataProvider::glyph_names(&font);
                // Synthesized names are `gidDDD` placeholders invented when the
                // font carries neither a `post` table nor a CFF charset. They
                // are not glyph names and must never reach the §9.10.2
                // Priority 3c AGL lookup, which would map them to nothing (or,
                // worse, to whatever a `uniXXXX`-shaped placeholder resembles). ~keep
                if glyph_names.source() == skrifa::GlyphNameSource::Synthesized {
                    tracing::debug!(
                        "Font '{}': embedded program has no usable glyph names (post Format 3 or stripped)",
                        self.base_font
                    );
                    return None;
                }
                let n = u16::try_from(glyph_names.num_glyphs()).unwrap_or(u16::MAX);
                let mut out: Vec<Option<String>> = Vec::with_capacity(n as usize);
                let mut found_any = false;
                for g in 0..n {
                    let name = glyph_names
                        .get(skrifa::GlyphId::from(g))
                        .filter(|name| !name.is_synthesized())
                        .map(|name| name.as_str().to_string())
                        .filter(|s| !s.is_empty() && s != ".notdef");
                    if name.is_some() {
                        found_any = true;
                    }
                    out.push(name);
                }
                if !found_any {
                    tracing::debug!(
                        "Font '{}': embedded program has no usable glyph names (post Format 3 or stripped)",
                        self.base_font
                    );
                    return None;
                }
                tracing::debug!(
                    "Font '{}': cached {} embedded glyph names (post/charset) for §9.10.2 Priority 3c fallback",
                    self.base_font,
                    out.iter().filter(|n| n.is_some()).count(),
                );
                Some(out)
            })
            .as_ref()?;
        names.get(gid as usize).and_then(|n| n.as_deref())
    }

    /// Authoritative glyph name for a *simple* font character code, in priority
    /// order (ISO 32000-1 §9.6.6.1 / §9.10.2):
    /// (a) the `/Differences` glyph name retained in `diff_glyph_names`;
    /// (b) else the embedded post/charset glyph name for the code's GID
    ///     (`embedded_glyph_name`), when the embedded program carries names.
    ///
    /// Used by the Item 1 punctuation-recovery interceptions in `char_to_unicode`.
    fn glyph_name_for_code(&self, char_code: u32) -> Option<&str> {
        if let Some(name) = self.diff_glyph_names.get(&(char_code as u8)) {
            return Some(name.as_str());
        }
        // Fall back to the embedded program's glyph name for this code's GID.
        // For embedded CFF subsets the byte_code → GID map is authoritative;
        // otherwise treat the code as the GID (TrueType simple-font convention). ~keep
        let gid = self
            .cff_gid_map
            .as_ref()
            .and_then(|m| m.get(&(char_code as u8)).copied())
            .unwrap_or(char_code as u16);
        self.embedded_glyph_name(gid)
    }

    /// Parse font information from a font dictionary object.
    ///
    /// # Arguments
    ///
    /// * `dict` - The font dictionary object (should be a Dictionary or Stream)
    /// * `doc` - The PDF document (needed to load referenced objects)
    ///
    /// # Returns
    ///
    /// A FontInfo struct containing the parsed font information.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The object is not a dictionary
    /// - Required font dictionary entries are missing or invalid
    /// - Referenced objects cannot be loaded
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use xberg_native_pdf::document::PdfDocument;
    /// use xberg_native_pdf::fonts::FontInfo;
    /// use xberg_native_pdf::object::ObjectRef;
    ///
    /// # fn example(doc: PdfDocument, font_ref: ObjectRef) -> Result<(), Box<dyn std::error::Error>> {
    /// let font_obj = doc.load_object(font_ref)?;
    /// let font_info = FontInfo::from_dict(&font_obj, &doc)?;
    /// println!("Font: {}", font_info.base_font);
    /// # Ok(())
    /// # }
    /// ```
    pub fn from_dict(dict: &Object, doc: &PdfDocument) -> Result<Self> {
        let font_dict = dict.as_dict().ok_or_else(|| Error::ParseError {
            offset: 0,
            reason: "Font object is not a dictionary".to_string(),
        })?;

        let base_font = font_dict
            .get("BaseFont")
            .and_then(|obj| obj.as_name())
            .unwrap_or("Unknown")
            .to_string();

        let subtype = font_dict
            .get("Subtype")
            .and_then(|obj| obj.as_name())
            .unwrap_or("Unknown")
            .to_string();

        if subtype == "Type3" {
            let msg = format!(
                "Font '{}' is Type 3 - may require special glyph name mapping",
                base_font
            );
            tracing::warn!(
                target: crate::LOG_TARGET_ROOT,
                operation = "load_font",
                error_code = "type3_font",
                "using Type 3 glyph-name fallback"
            );
            // push into the structured warning
            // sink. PDF Spec §9.6.4 "Type 3 Fonts" describes the
            // user-defined CharProcs glyph-program model; the
            // standard glyph name registry doesn't apply, so
            // extraction may fall back to glyph-name heuristics. ~keep
            crate::extractors::warnings::push_global_warning(crate::extractors::warnings::Warning {
                category: crate::extractors::warnings::WarningCategory::Type3Font,
                page: None,
                message: msg,
                spec_section: Some("9.6.4"),
            });
        }

        // Parse FontMatrix [a] for Type 3 fonts.
        // Standard Type 1 FontMatrix is [0.001 0 0 0.001 0 0], so widths are in 1/1000 em.
        // Type 3 fonts can use an identity FontMatrix [1 0 0 1 0 0], meaning widths are
        // in text-space units directly (no 1/1000 scaling needed). ~keep
        let font_matrix_a = if subtype == "Type3" {
            font_dict
                .get("FontMatrix")
                .and_then(|obj| obj.as_array())
                .and_then(|arr| arr.first())
                .and_then(|v| {
                    v.as_real()
                        .map(|r| r as f32)
                        .or_else(|| v.as_integer().map(|i| i as f32))
                })
                // A degenerate FontMatrix[0] — zero, near-zero, or non-finite —
                // is a malformed horizontal scale (ISO 32000-1 §9.2.4 / §9.6.5)
                // and would make the `default_width * 0.001 / font_matrix_a`
                // rescale below divide by ~0 → inf/NaN, and the
                // `font_size * font_matrix_a` advance collapse to 0. Reject it
                // and fall back to the standard 0.001 (Type 1) scale. ~keep
                .filter(|a| a.is_finite() && a.abs() > 1e-6)
                .unwrap_or(0.001)
        } else {
            0.001
        };

        // Parse FontDescriptor FIRST to get font flags (needed for encoding decision)
        // PDF Spec: ISO 32000-1:2008, Section 9.6.2 - Font Descriptor ~keep
        let (
            font_weight,
            flags,
            stem_v,
            mut embedded_font_data,
            is_truetype_font,
            raw_ascent,
            raw_descent,
            mut has_font_program,
        ) = Self::resolve_descriptor_fields(font_dict, doc, &base_font);

        // TrueType cmap extraction is now LAZY — deferred until first access via
        // truetype_cmap() accessor. This saves 10-25ms per font when ToUnicode CMap
        // (Priority 1) resolves all characters, making the cmap unnecessary.
        // The is_truetype_font flag is recorded here for the lazy accessor to use. ~keep

        // Parse encoding (now that we have flags)
        // PDF Spec: ISO 32000-1:2008, Section 9.6.6.1
        // "For symbolic fonts, the Encoding entry is ignored"
        //
        // However, many PDF generators (LaTeX, LibreOffice, etc.) incorrectly set the
        // Symbolic flag on non-symbolic fonts. When an explicit /Encoding entry exists,
        // we always parse it — real-world PDF viewers (MuPDF, poppler, pdf.js) do the same.
        // The Symbolic flag only controls behavior when NO /Encoding is present.
        // Pre-parse font program encoding (needed for /Differences base encoding per PDF spec)
        // ~keep
        let font_program_enc_cache: Option<HashMap<u8, char>> = if let Some(font_data) = &embedded_font_data {
            if subtype == "Type1" || subtype == "MMType1" {
                super::type1_encoding::parse_type1_encoding(font_data)
            } else {
                super::cff_encoding::parse_cff_encoding(font_data)
            }
        } else {
            None
        };

        let (encoding_wmode, encoding, diff_multi_char_map, diff_glyph_names) =
            Self::resolve_encoding_fields(font_dict, doc, &base_font, flags, font_program_enc_cache)?;

        // Parse ToUnicode CMap if present (Phase 5.1: Lazy Loading)
        // The CMap stream is stored raw and parsed only on first character lookup ~keep
        let to_unicode = if let Some(cmap_ref) = font_dict.get("ToUnicode").and_then(|obj| obj.as_reference()) {
            let stream_opt = match doc.load_object(cmap_ref) {
                Ok(cmap_obj) => match doc.decode_stream_with_encryption(&cmap_obj, cmap_ref) {
                    Ok(data) => Some(data),
                    Err(error) => {
                        crate::error::trace_recovery("decode_tounicode_cmap", &error);
                        None
                    }
                },
                Err(error) => {
                    crate::error::trace_recovery("load_tounicode_cmap", &error);
                    None
                }
            };

            if let Some(stream_bytes) = stream_opt {
                // Store raw bytes for lazy parsing — LazyCMap handles errors on first access.
                // Skipping eager validation avoids parsing every CMap twice. ~keep
                tracing::debug!(
                    "ToUnicode CMap stream loaded for font '{}': {} bytes (lazy parsing enabled)",
                    base_font,
                    stream_bytes.len()
                );
                Some(LazyCMap::new_for_font(stream_bytes, base_font.clone()))
            } else {
                // Specific error already logged above in the match arms ~keep
                None
            }
        } else {
            if subtype == "Type0" {
                let msg = format!("Type0 font '{}' has no ToUnicode entry!", base_font);
                tracing::warn!(
                    target: crate::LOG_TARGET_ROOT,
                    operation = "load_font",
                    error_code = "missing_tounicode",
                    "using composite-font fallback mapping"
                );
                // push to the structured sink. PDF
                // Spec §9.10.2 "ToUnicode CMaps" describes the
                // mapping; absent ToUnicode triggers the fallback
                // chain (Encoding → AGL → CID-as-Unicode) per §9.10.3. ~keep
                crate::extractors::warnings::push_global_warning(crate::extractors::warnings::Warning {
                    category: crate::extractors::warnings::WarningCategory::ToUnicodeMissing,
                    page: None,
                    message: msg,
                    spec_section: Some("9.10.2"),
                });
            }
            None
        };

        // Parse /Widths array for glyph width information
        // PDF Spec: ISO 32000-1:2008, Section 9.7.4 - Font Widths
        //
        // For simple fonts (Type1, TrueType), widths are specified as an array
        // of integers in 1000ths of em, indexed from FirstChar to LastChar.
        //
        // Note: Type0 (CID) fonts use a different /W array format, parsed via parse_descendant_fonts below
        // ~keep
        let (widths, first_char, last_char) = if subtype != "Type0" {
            let widths_opt = font_dict.get("Widths").and_then(|widths_obj| {
                let resolved = if let Some(ref_obj) = widths_obj.as_reference() {
                    doc.load_object(ref_obj).ok()?
                } else {
                    widths_obj.clone()
                };

                resolved.as_array().map(|arr| {
                    arr.iter()
                        .filter_map(|obj| {
                            obj.as_integer()
                                .map(|i| i as f32)
                                .or_else(|| obj.as_real().map(|r| r as f32))
                        })
                        .collect::<Vec<f32>>()
                })
            });

            let first = font_dict
                .get("FirstChar")
                .and_then(|obj| obj.as_integer())
                .map(|i| i as u32);

            let last = font_dict
                .get("LastChar")
                .and_then(|obj| obj.as_integer())
                .map(|i| i as u32);

            if widths_opt.is_some() {
                tracing::debug!(
                    "Font '{}': parsed {} widths (FirstChar={:?}, LastChar={:?})",
                    base_font,
                    widths_opt.as_ref().map(|w| w.len()).unwrap_or(0),
                    first,
                    last
                );
            } else {
                tracing::debug!("Font '{}': no /Widths array found, will use default width", base_font);
            }

            (widths_opt, first, last)
        } else {
            tracing::debug!("Font '{}': Type0 font, widths parsed from CIDFont /W array", base_font);
            (None, None, None)
        };

        // Set default width based on font characteristics
        // PDF Spec: Typical values are 500-600 for proportional fonts, ~600 for monospace
        // ~keep
        let default_width = if let Some(flags_val) = flags {
            const FIXED_PITCH_BIT: i32 = 1 << 0;
            if (flags_val & FIXED_PITCH_BIT) != 0 {
                600.0
            } else {
                500.0
            }
        } else {
            550.0
        };

        // The heuristic above is calibrated for standard fonts where font_matrix_a = 0.001
        // (i.e. glyph-space units are 1/1000 em).  Type3 fonts can use an arbitrary
        // FontMatrix; if font_matrix_a differs from 0.001, rescale so that callers
        // multiplying by font_matrix_a still get the intended em-fraction result. ~keep
        let default_width = if subtype == "Type3" && font_matrix_a != 0.001 {
            default_width * 0.001 / font_matrix_a
        } else {
            default_width
        };

        let (
            cid_to_gid_map,
            cid_system_info,
            cid_font_type,
            cid_widths,
            cid_default_width,
            has_explicit_dw,
            descendant_tt_cmap,
            desc_raw_ascent,
            desc_raw_descent,
            cid_vertical_metrics,
            cid_default_vertical_metrics,
        ) = if subtype == "Type0" {
            match Self::parse_descendant_fonts(font_dict, &base_font, doc) {
                Ok((
                    map,
                    info,
                    ftype,
                    widths,
                    dw,
                    explicit_dw,
                    tt_cmap,
                    (desc_has_font_program, desc_embedded),
                    d_ascent,
                    d_descent,
                    vmetrics,
                    dvmetrics,
                )) => {
                    tracing::debug!(
                        "Font '{}': Parsed DescendantFonts - CIDFontType={}, CIDSystemInfo={}-{}, widths={}, embedded={}",
                        base_font,
                        ftype.as_ref().unwrap_or(&"Unknown".to_string()),
                        info.as_ref().map(|s| s.registry.as_str()).unwrap_or("Unknown"),
                        info.as_ref().map(|s| s.ordering.as_str()).unwrap_or("Unknown"),
                        widths.as_ref().map(|m| m.len()).unwrap_or(0),
                        desc_embedded.is_some()
                    );
                    if desc_embedded.is_some() && embedded_font_data.is_none() {
                        embedded_font_data = desc_embedded;
                    }
                    has_font_program |= desc_has_font_program;
                    (
                        map,
                        info,
                        ftype,
                        widths,
                        dw,
                        explicit_dw,
                        tt_cmap,
                        d_ascent,
                        d_descent,
                        vmetrics,
                        dvmetrics,
                    )
                }
                Err(error) => {
                    crate::error::trace_recovery("parse_descendant_fonts", &error);
                    (
                        Some(CIDToGIDMap::Identity),
                        None,
                        None,
                        None,
                        1000.0,
                        false,
                        None,
                        None,
                        None,
                        None,
                        VerticalMetrics::SPEC_DEFAULT,
                    )
                }
            }
        } else {
            (
                None,
                None,
                None,
                None,
                1000.0,
                false,
                None,
                None,
                None,
                None,
                VerticalMetrics::SPEC_DEFAULT,
            )
        };

        // For Type0 fonts the /FontDescriptor lives on the CIDFont descendant (§9.7.4).
        // If the top-level font had no descriptor (the common case), fall back to the
        // descendant's values so CID/CJK glyphs get real metrics instead of the 0.95/-0.35
        // Poppler-compatible default. ~keep
        let raw_ascent = raw_ascent.or(desc_raw_ascent);
        let raw_descent = raw_descent.or(desc_raw_descent);

        let truetype_cmap_lock = std::sync::OnceLock::new();
        if let Some(desc_cmap) = descendant_tt_cmap {
            let _ = truetype_cmap_lock.set(Some(desc_cmap));
        }

        // Parse CFF GID mapping ONLY for simple (non-Type0) fonts with embedded CFF data.
        // Type0/CID fonts use Identity-H encoding and CIDToGIDMap, not CFF Standard Encoding.
        //
        // §9.6.6: the byte → GID resolution must use the PDF font dictionary's
        // /Encoding as the byte → glyph-name source and the CFF Charset as the
        // glyph-name → GID resolver. Subsetter-emitted custom CFF Encoding
        // tables are frequently sparse (some prepress subsetters emit only
        // `space` and `A`) and would silently drop most content bytes to
        // `.notdef` without this routing. ~keep
        let cff_gid_map = if subtype != "Type0" {
            embedded_font_data.as_ref().and_then(|data| {
                super::cff_encoding::parse_cff_gid_mapping_with_pdf_encoding(data, &encoding, &diff_glyph_names)
                    .inspect(|map| {
                        tracing::debug!(
                            "Font '{}': parsed CFF GID mapping via PDF /Encoding ({} entries)",
                            base_font,
                            map.len()
                        );
                    })
            })
        } else {
            None
        };

        // Normalize ascent/descent from 1000ths-of-em to fraction-of-em.
        // PDF spec says these are in 1/1000 of em (glyph space units).
        // Fall back to standard font metrics for the 14 standard PDF fonts,
        // then to Poppler-compatible defaults (0.95 / -0.35). ~keep
        let (default_ascent, default_descent) = standard_font_metrics(&base_font).unwrap_or((0.95, -0.35));
        let ascent = raw_ascent.map(|v| v / 1000.0).unwrap_or(default_ascent);
        // PDF Descent should be ≤ 0 (below baseline). Some PDFs store it as a positive
        // magnitude; Poppler normalizes by negating. Mirror that here. ~keep
        let descent = raw_descent
            .map(|v| {
                let d = v / 1000.0;
                if d > 0.0 { -d } else { d }
            })
            .unwrap_or(default_descent);

        // Final writing-mode resolution.
        //
        // Per ISO 32000-1:2008 §9.10.2 the ToUnicode CMap is for
        // extraction-time character → Unicode mapping ONLY. The active
        // writing mode is determined by the /Encoding CMap (§9.7.5):
        // either an embedded `/WMode 1 def` directive or a predefined
        // encoding name whose suffix is `-V`. Consulting the ToUnicode
        // CMap's `/WMode` here would silently flip a horizontal document
        // to vertical whenever a producer left a stale `/WMode 1 def`
        // in the ToUnicode prologue — a real-world tooling failure mode.
        //
        // We still emit a debug log when ToUnicode disagrees with the
        // /Encoding so producer bugs are diagnosable. ~keep
        let wmode = encoding_wmode;
        if let Some(tu) = to_unicode.as_ref() {
            let tu_wmode = tu.wmode();
            if tu_wmode != encoding_wmode {
                tracing::debug!(
                    "Font '{}': ToUnicode CMap declares /WMode {} but /Encoding wmode is {}. \
                     Honoring /Encoding per ISO 32000-1 §9.10.2.",
                    base_font,
                    tu_wmode,
                    encoding_wmode
                );
            }
        }

        // Detect Adobe predefined CIDFont substitution candidates.
        // Conditions (all must hold):
        //   1. Type0 font (the only place predefined CMaps are referenced).
        //   2. No embedded font program — neither the Type0 wrapper's nor the
        //      CIDFont descendant's FontDescriptor carries a `/FontFile{,2,3}`
        //      KEY. Key presence (not extraction success) is what gates here:
        //      a present-but-undecodable program means the document embeds its
        //      own outlines and the decode failure must surface as a warning,
        //      not be masked by a silent sans-serif substitution.
        //   3. The /Encoding resolves to an Identity charcode→CID mapping
        //      (Identity-H/V or an Adobe-collection identity CMap stream).
        //      Non-Identity predefined CMaps (90ms-RKSJ-H, GBK-EUC-H, …) carry
        //      raw legacy multi-byte codes, not CIDs — substituting would
        //      index the CID→Unicode tables with Shift-JIS / EUC values and
        //      paint wrong glyphs. Those CMaps stay unsubstituted until a
        //      charcode→CID CMap pass is wired.
        //   4. The base font name (after subset-prefix + CMap-suffix strip)
        //      matches one of the registered predefined names from
        //      Technical Notes #5078 / #5079 / #5080 / #5093.
        // The character collection comes from the descendant's /CIDSystemInfo
        // Ordering when it names a known collection (it is authoritative for
        // CID semantics per ISO 32000-1 §9.7.3); the name-derived collection
        // is the fallback for Identity/unknown orderings.
        // When all hold, the renderer routes the paint through the bundled
        // covering font; otherwise we leave `cjk_substitution` at `None` and
        // the existing render path runs unchanged. ~keep
        let cjk_substitution = if subtype == "Type0"
            && !has_font_program
            && embedded_font_data.is_none()
            && matches!(encoding, Encoding::Identity)
        {
            use super::predefined_cidfont::CharacterCollection;
            let name_collection = super::predefined_cidfont::is_predefined(&base_font);
            let ordering_collection = cid_system_info.as_ref().and_then(|info| match info.ordering.as_str() {
                "Japan1" => Some(CharacterCollection::AdobeJapan1),
                "GB1" => Some(CharacterCollection::AdobeGB1),
                "CNS1" => Some(CharacterCollection::AdobeCNS1),
                "Korea1" => Some(CharacterCollection::AdobeKorea1),
                _ => None,
            });
            let collection = match (name_collection, ordering_collection) {
                (Some(n), Some(o)) if n != o => {
                    tracing::debug!(
                        "Font '{}': base name implies collection {:?} but \
                         /CIDSystemInfo Ordering says {:?}; trusting CIDSystemInfo",
                        base_font,
                        n,
                        o
                    );
                    Some(o)
                }
                (Some(n), _) => Some(n),
                (None, _) => None,
            };
            if collection.is_some() {
                tracing::warn!(
                    target: crate::LOG_TARGET_ROOT,
                    operation = "load_cid_font",
                    error_code = "predefined_font_substitution",
                    "using bundled predefined CID font"
                );
            }
            collection
        } else {
            if subtype == "Type0" && super::predefined_cidfont::is_predefined(&base_font).is_some() {
                if has_font_program && embedded_font_data.is_none() {
                    tracing::warn!(
                        target: crate::LOG_TARGET_ROOT,
                        operation = "load_cid_font",
                        error_code = "embedded_font_unavailable",
                        "skipping predefined CID font substitution"
                    );
                } else if !has_font_program && !matches!(encoding, Encoding::Identity) {
                    tracing::warn!(
                        target: crate::LOG_TARGET_ROOT,
                        operation = "load_cid_font",
                        error_code = "unsupported_predefined_encoding",
                        "skipping predefined CID font substitution"
                    );
                }
            }
            None
        };

        Ok(FontInfo {
            base_font,
            subtype,
            encoding,
            to_unicode,
            font_weight,
            flags,
            stem_v,
            ascent,
            descent,
            embedded_font_data,
            truetype_cmap: truetype_cmap_lock,
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font,
            cid_to_gid_map,
            cid_system_info,
            cid_font_type,
            font_matrix_a,
            widths,
            first_char,
            last_char,
            default_width,
            cid_widths,
            cid_default_width,
            has_explicit_dw,
            cff_gid_map,
            multi_char_map: diff_multi_char_map,
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names,
            wmode,
            cid_vertical_metrics,
            cid_default_vertical_metrics,
            cjk_substitution,
        })
    }

    /// Resolves the FontDescriptor-derived fields used by [`Self::from_dict`]: font
    /// weight, flags, StemV, the embedded font program (if any), whether that program
    /// is TrueType, raw ascent/descent, and whether any font-file key is present at
    /// all (even if its program failed to decode). Code moved verbatim out of
    /// `from_dict`; every failure path (missing descriptor, unresolved reference,
    /// load error, non-dictionary descriptor) returns the same all-default tuple the
    /// inline version did. ~keep
    fn resolve_descriptor_fields(
        font_dict: &HashMap<String, Object>,
        doc: &PdfDocument,
        base_font: &str,
    ) -> (
        Option<i32>,
        Option<i32>,
        Option<f32>,
        Option<Arc<Vec<u8>>>,
        bool,
        Option<f32>,
        Option<f32>,
        bool,
    ) {
        let defaults = (None, None, None, None, false, None, None, false);

        let Some(descriptor_ref) = font_dict.get("FontDescriptor").and_then(|obj| obj.as_reference()) else {
            return defaults;
        };
        let Ok(descriptor_obj) = doc.load_object(descriptor_ref) else {
            return defaults;
        };
        let Some(descriptor_dict) = descriptor_obj.as_dict() else {
            return defaults;
        };

        let weight = descriptor_dict
            .get("FontWeight")
            .and_then(|weight_obj| weight_obj.as_integer())
            .map(|w| w as i32);

        let descriptor_flags = descriptor_dict
            .get("Flags")
            .and_then(|flags_obj| flags_obj.as_integer())
            .map(|f| f as i32);

        let stem_v_value = descriptor_dict.get("StemV").and_then(|sv_obj| {
            sv_obj
                .as_real()
                .map(|r| r as f32)
                .or_else(|| sv_obj.as_integer().map(|i| i as f32))
        });

        let ascent_value = descriptor_dict.get("Ascent").and_then(|obj| {
            obj.as_real()
                .map(|r| r as f32)
                .or_else(|| obj.as_integer().map(|i| i as f32))
        });

        let descent_value = descriptor_dict.get("Descent").and_then(|obj| {
            obj.as_real()
                .map(|r| r as f32)
                .or_else(|| obj.as_integer().map(|i| i as f32))
        });

        // Load embedded font data from FontFile2 (TrueType), FontFile (Type 1), or FontFile3 (CFF/OpenType)
        // IMPORTANT: Track whether font is TrueType or CFF - only TrueType fonts have cmaps!
        // ~keep
        //
        // Key presence is recorded separately from extraction
        // success: a present-but-undecodable font program means
        // the document intended to be self-contained, which
        // downstream gates (CJK predefined-CIDFont substitution)
        // must distinguish from "no program at all". ~keep
        let has_font_program = descriptor_dict.contains_key("FontFile2")
            || descriptor_dict.contains_key("FontFile3")
            || descriptor_dict.contains_key("FontFile");
        let (embedded_font, is_truetype_font) = if let Some(ff2_obj) = descriptor_dict.get("FontFile2") {
            tracing::debug!("Font '{}' has FontFile2 entry (TrueType)", base_font);
            let font_data = ff2_obj
                .as_reference()
                .and_then(|ff2_ref| {
                    doc.load_object(ff2_ref)
                        .inspect_err(|error| {
                            crate::error::trace_recovery("load_truetype_font_program", error);
                        })
                        .ok()
                        .map(|obj| (obj, ff2_ref))
                })
                .and_then(|(ff2_stream, ff2_ref)| {
                    doc.decode_stream_with_encryption(&ff2_stream, ff2_ref)
                        .inspect_err(|error| {
                            crate::error::trace_recovery("decode_truetype_font_program", error);
                        })
                        .ok()
                })
                .map(|data| {
                    tracing::debug!(
                        "Font '{}' loaded embedded TrueType font ({} bytes)",
                        base_font,
                        data.len()
                    );
                    Arc::new(data)
                });
            (font_data, true)
        } else if let Some(ff3_obj) = descriptor_dict.get("FontFile3") {
            tracing::debug!(
                "Font '{}' has FontFile3 entry (CFF/OpenType - no TrueType cmap)",
                base_font
            );
            let font_data = ff3_obj
                .as_reference()
                .and_then(|ff3_ref| {
                    doc.load_object(ff3_ref)
                        .inspect_err(|error| {
                            crate::error::trace_recovery("load_cff_font_program", error);
                        })
                        .ok()
                        .map(|obj| (obj, ff3_ref))
                })
                .and_then(|(ff3_stream, ff3_ref)| {
                    doc.decode_stream_with_encryption(&ff3_stream, ff3_ref)
                        .inspect_err(|error| {
                            crate::error::trace_recovery("decode_cff_font_program", error);
                        })
                        .ok()
                })
                .map(|data| {
                    let data = if !data.is_empty() && data[0] == 1 && data.len() > 4 {
                        tracing::debug!(
                            "Font '{}': Wrapping raw CFF in OpenType ({} bytes)",
                            base_font,
                            data.len()
                        );
                        wrap_cff_in_opentype(&data)
                    } else {
                        tracing::debug!(
                            "Font '{}' loaded embedded CFF/OpenType font ({} bytes)",
                            base_font,
                            data.len()
                        );
                        data
                    };
                    Arc::new(data)
                });
            (font_data, false)
        } else if let Some(ff_obj) = descriptor_dict.get("FontFile") {
            tracing::debug!("Font '{}' has FontFile entry (Type 1)", base_font);
            let font_data = ff_obj
                .as_reference()
                .and_then(|ff_ref| {
                    doc.load_object(ff_ref)
                        .inspect_err(|error| {
                            crate::error::trace_recovery("load_type1_font_program", error);
                        })
                        .ok()
                        .map(|obj| (obj, ff_ref))
                })
                .and_then(|(ff_stream, ff_ref)| {
                    doc.decode_stream_with_encryption(&ff_stream, ff_ref)
                        .inspect_err(|error| {
                            crate::error::trace_recovery("decode_type1_font_program", error);
                        })
                        .ok()
                })
                .map(|data| {
                    tracing::debug!(
                        "Font '{}' loaded embedded Type 1 font ({} bytes)",
                        base_font,
                        data.len()
                    );
                    Arc::new(data)
                });
            (font_data, false)
        } else {
            tracing::debug!("Font '{}' has no embedded font data", base_font);
            (None, false)
        };

        (
            weight,
            descriptor_flags,
            stem_v_value,
            embedded_font,
            is_truetype_font,
            ascent_value,
            descent_value,
            has_font_program,
        )
    }

    /// Resolves the encoding-related fields used by [`Self::from_dict`]: the writing
    /// mode, the [`Encoding`], and the two `/Differences`-derived side maps. Code moved
    /// verbatim out of `from_dict`; `encoding_wmode` was previously mutated in place via
    /// a captured local and is now returned instead — same value, same only-set-when-an-
    /// `/Encoding` entry-is-present behavior. ~keep
    fn resolve_encoding_fields(
        font_dict: &HashMap<String, Object>,
        doc: &PdfDocument,
        base_font: &str,
        flags: Option<i32>,
        font_program_enc_cache: Option<HashMap<u8, char>>,
    ) -> Result<(u8, Encoding, HashMap<u8, String>, HashMap<u8, String>)> {
        fn is_symbolic_font(flags_opt: Option<i32>, base_font: &str) -> bool {
            if let Some(flags_value) = flags_opt {
                const SYMBOLIC_BIT: i32 = 1 << 2;
                (flags_value & SYMBOLIC_BIT) != 0
            } else {
                let name_lower = base_font.to_lowercase();
                name_lower.contains("symbol") || name_lower.contains("zapf") || name_lower.contains("dingbat")
            }
        }

        // Writing-mode signal sourced from the encoding object. Resolved
        // here because the `Encoding` enum collapses `Identity-H` and
        // `Identity-V` to the same `Encoding::Identity` variant — we need
        // the original name to recover wmode. Defaults to `0` (horizontal)
        // when no encoding object is present. ~keep
        let mut encoding_wmode: u8 = 0;
        let (encoding, diff_multi_char_map, diff_glyph_names) = if let Some(enc_obj) = font_dict.get("Encoding") {
            let resolved_enc_obj = if let Some(obj_ref) = enc_obj.as_reference() {
                doc.load_object(obj_ref)?
            } else {
                enc_obj.clone()
            };

            // Inspect for `-V` predefined name or embedded `/WMode 1 def`
            // before parse_encoding flattens the variant. ~keep
            let (_enc_name, wm) = Self::resolve_encoding_writing_mode(&resolved_enc_obj, doc);
            encoding_wmode = wm;

            if is_symbolic_font(flags, base_font) {
                tracing::debug!(
                    "Font '{}' is symbolic (Flags={:?}) but has /Encoding — parsing it anyway (common in LaTeX/LibreOffice PDFs)",
                    base_font,
                    flags
                );
            } else {
                tracing::debug!("Font '{}' using /Encoding entry", base_font);
            }
            let (mut parsed_enc, mut multi_map, glyph_names) =
                Self::parse_encoding(&resolved_enc_obj, doc, font_program_enc_cache.as_ref())?;

            // When /Encoding is a named encoding (e.g., /WinAnsiEncoding) AND the font
            // has an embedded program, merge the font program's encoding. This handles
            // fonts where the program maps glyphs to non-standard code positions
            // (e.g., space at 0xCA) that the named encoding maps differently.
            // The font program's mappings override the standard encoding. ~keep
            if matches!(parsed_enc, Encoding::Standard(_))
                && let Some(prog_enc) = &font_program_enc_cache
            {
                let std_name = match &parsed_enc {
                    Encoding::Standard(n) => n.clone(),
                    _ => "StandardEncoding".to_string(),
                };

                // Decide whether the embedded program's built-in encoding is a
                // meaningful text encoding (a few non-standard slots to overlay,
                // e.g. space at 0xCA) or a re-indexed *cipher* — a subset font's
                // own glyph ordering that bears no relation to the producer's
                // declared named base encoding. Overlaying a cipher rewrites every
                // mapped code into mojibake. Discriminate by agreement: count how
                // many program codes resolve to the SAME character the named base
                // already gives. A real encoding agrees on most; a cipher on
                // almost none. ~keep
                let looks_like_cipher = builtin_encoding_looks_like_cipher(prog_enc, &std_name);

                if looks_like_cipher {
                    tracing::debug!(
                        "Font '{base_font}': built-in encoding disagrees with {std_name} on most overlapping codes — treating as a subset cipher and keeping the named encoding"
                    );
                } else {
                    tracing::debug!(
                        "Font '{}': merging {} font program encoding entries with {}",
                        base_font,
                        prog_enc.len(),
                        std_name,
                    );
                    let mut custom_map: HashMap<u8, char> = HashMap::new();
                    for code in 0u8..=255 {
                        let Some(unicode_str) = standard_encoding_lookup(&std_name, code) else {
                            continue;
                        };
                        let Some(ch) = unicode_str.chars().next() else {
                            continue;
                        };
                        custom_map.insert(code, ch);
                    }
                    for (&code, &ch) in prog_enc {
                        custom_map.insert(code, ch);
                        if !is_ligature_char(ch) {
                            continue;
                        }
                        let Some(expanded) = expand_ligature_char(ch) else {
                            continue;
                        };
                        multi_map.insert(code, expanded.to_string());
                    }
                    parsed_enc = Encoding::Custom(custom_map);
                }
            }

            (parsed_enc, multi_map, glyph_names)
        } else if let Some(prog_enc) = font_program_enc_cache {
            tracing::debug!(
                "Font '{}' using built-in font program encoding ({} mappings)",
                base_font,
                prog_enc.len()
            );
            let mut multi_map: HashMap<u8, String> = HashMap::new();
            for (&code, &ch) in &prog_enc {
                if !is_ligature_char(ch) {
                    continue;
                }
                let Some(expanded) = expand_ligature_char(ch) else {
                    continue;
                };
                multi_map.insert(code, expanded.to_string());
            }
            (Encoding::Custom(prog_enc), multi_map, HashMap::new())
        } else if is_symbolic_font(flags, base_font) {
            tracing::debug!(
                "Font '{}' is symbolic with no /Encoding - will use built-in encoding (Symbol/ZapfDingbats)",
                base_font
            );
            (
                Encoding::Standard("SymbolicBuiltIn".to_string()),
                HashMap::new(),
                HashMap::new(),
            )
        } else {
            tracing::debug!(
                "Font '{}' has no /Encoding entry - defaulting to StandardEncoding",
                base_font
            );
            (
                Encoding::Standard("StandardEncoding".to_string()),
                HashMap::new(),
                HashMap::new(),
            )
        };

        Ok((encoding_wmode, encoding, diff_multi_char_map, diff_glyph_names))
    }

    /// Parse encoding from an encoding object.
    ///
    /// Phase 3: Parse CIDSystemInfo from CIDFont dictionary
    /// Extracts Registry, Ordering, and Supplement for character collection identification
    /// Per PDF Spec ISO 32000-1:2008, Section 9.7.3
    fn parse_cidsysteminfo(cidfont_dict: &HashMap<String, Object>, doc: &PdfDocument) -> Result<CIDSystemInfo> {
        let sysinfo_obj = cidfont_dict.get("CIDSystemInfo").ok_or_else(|| Error::ParseError {
            offset: 0,
            reason: "CIDFont missing required /CIDSystemInfo entry".to_string(),
        })?;

        let resolved = if let Some(ref_obj) = sysinfo_obj.as_reference() {
            doc.load_object(ref_obj)?
        } else {
            sysinfo_obj.clone()
        };

        let sysinfo_dict = resolved.as_dict().ok_or_else(|| Error::ParseError {
            offset: 0,
            reason: "CIDSystemInfo is not a dictionary".to_string(),
        })?;

        let registry = sysinfo_dict
            .get("Registry")
            .and_then(|obj| obj.as_string())
            .map(|s| String::from_utf8_lossy(s).to_string())
            .unwrap_or_else(|| "Unknown".to_string());

        let ordering = sysinfo_dict
            .get("Ordering")
            .and_then(|obj| obj.as_string())
            .map(|s| String::from_utf8_lossy(s).to_string())
            .unwrap_or_else(|| "Unknown".to_string());

        let supplement = sysinfo_dict
            .get("Supplement")
            .and_then(|obj| obj.as_integer())
            .unwrap_or(0) as i32;

        tracing::debug!(
            "CIDSystemInfo parsed: Registry={}, Ordering={}, Supplement={}",
            registry,
            ordering,
            supplement
        );

        Ok(CIDSystemInfo {
            registry,
            ordering,
            supplement,
        })
    }

    /// Phase 3: Parse DescendantFonts array for Type0 fonts
    /// Extracts CIDFont dictionary and related information
    /// Per PDF Spec ISO 32000-1:2008, Section 9.7.1
    ///
    /// Returns: (CIDToGIDMap, CIDSystemInfo, CIDFontType, CIDWidths, DefaultWidth,
    ///          has_explicit_dw, TrueTypeCMap, (has_font_program, EmbeddedFontData),
    ///          raw_ascent, raw_descent, vertical_metrics, dw2)
    ///
    /// The embedded-font element pairs descriptor `/FontFile{,2,3}` key
    /// presence with the extracted bytes so callers can tell "no program"
    /// apart from "program present but failed to load/decode".
    #[allow(clippy::type_complexity)]
    fn parse_descendant_fonts(
        font_dict: &HashMap<String, Object>,
        base_font: &str,
        doc: &PdfDocument,
    ) -> Result<(
        Option<CIDToGIDMap>,
        Option<CIDSystemInfo>,
        Option<String>,
        Option<HashMap<u16, f32>>,
        f32,
        bool,
        Option<TrueTypeCMap>,
        (bool, Option<Arc<Vec<u8>>>),
        Option<f32>,
        Option<f32>,
        Option<HashMap<u16, VerticalMetrics>>,
        VerticalMetrics,
    )> {
        let descendant_obj = font_dict.get("DescendantFonts").ok_or_else(|| Error::ParseError {
            offset: 0,
            reason: format!("Type0 font '{}' missing required /DescendantFonts entry", base_font),
        })?;

        let resolved = if let Some(ref_obj) = descendant_obj.as_reference() {
            doc.load_object(ref_obj)?
        } else {
            descendant_obj.clone()
        };

        let array = resolved.as_array().ok_or_else(|| Error::ParseError {
            offset: 0,
            reason: format!("Type0 font '{}': DescendantFonts is not an array", base_font),
        })?;

        if array.is_empty() {
            return Err(Error::ParseError {
                offset: 0,
                reason: format!(
                    "Type0 font '{}': DescendantFonts array is empty - must have at least 1 element",
                    base_font
                ),
            });
        }

        // Use first element (PDF spec: "Usually contains a single element") ~keep
        if array.len() > 1 {
            tracing::warn!(
                target: crate::LOG_TARGET_ROOT,
                operation = "parse_descendant_fonts",
                error_code = "extra_descendants",
                descendant_count = array.len(),
                "using first descendant font"
            );
        }

        // accept both indirect
        // references AND direct dictionary objects in DescendantFonts.
        // PDF spec §9.7.6 mandates indirect refs, but Persian / Farsi
        // PDFs from older XeTeX / pdfTeX writers (Nazanin, Yagut,
        // Mitra, Lotus fonts) commonly inline the CIDFont dict
        // directly. Older versions rejected the inline form with
        // "DescendantFonts[0] is not a reference" and fell back to
        // Identity-H, which emits CIDs as Latin-Extended-B garbage
        // instead of mapping through the CIDSystemInfo collection.
        // Accepting the inline form gets the parser past this gate;
        // bundling the official Adobe-Persian-1-UCS2 /
        // Adobe-Arabic-1-UCS2 CMap data is a separate follow-up. ~keep
        let cidfont_obj_owned;
        let cidfont_dict = match array[0].as_reference() {
            Some(cidfont_ref) => {
                cidfont_obj_owned = doc.load_object(cidfont_ref)?;
                cidfont_obj_owned.as_dict().ok_or_else(|| Error::ParseError {
                    offset: 0,
                    reason: format!("Type0 font '{}': CIDFont is not a dictionary", base_font),
                })?
            }
            None => {
                // Inline-dict path — accept it per §9.7.6 lenient
                // reader posture. ~keep
                tracing::warn!(
                    target: crate::LOG_TARGET_ROOT,
                    operation = "parse_descendant_fonts",
                    error_code = "inline_descendant",
                    "parsing inline descendant font"
                );
                array[0].as_dict().ok_or_else(|| Error::ParseError {
                    offset: 0,
                    reason: format!(
                        "Type0 font '{}': DescendantFonts[0] is neither a reference \
                         nor a dictionary",
                        base_font
                    ),
                })?
            }
        };

        let cid_font_type = cidfont_dict
            .get("Subtype")
            .and_then(|obj| obj.as_name())
            .ok_or_else(|| Error::ParseError {
                offset: 0,
                reason: format!("Type0 font '{}': CIDFont missing required /Subtype", base_font),
            })?
            .to_string();

        if cid_font_type != "CIDFontType0" && cid_font_type != "CIDFontType2" {
            return Err(Error::ParseError {
                offset: 0,
                reason: format!(
                    "Type0 font '{}': Invalid CIDFontType '{}' (must be CIDFontType0 or CIDFontType2)",
                    base_font, cid_font_type
                ),
            });
        }

        let cid_system_info = match Self::parse_cidsysteminfo(cidfont_dict, doc) {
            Ok(info) => Some(info),
            Err(error) => {
                crate::error::trace_recovery("parse_cid_system_info", &error);
                None
            }
        };

        let cid_to_gid_map = Self::resolve_cid_to_gid_map(cidfont_dict, &cid_font_type, base_font, doc);

        let dw_value = cidfont_dict.get("DW").and_then(|obj| {
            let resolved = if let Some(r) = obj.as_reference() {
                doc.load_object(r).ok()
            } else {
                Some(obj.clone())
            };
            resolved.and_then(|o| match &o {
                Object::Integer(i) => Some(*i as f32),
                Object::Real(r) => Some(*r as f32),
                _ => None,
            })
        });
        let has_explicit_dw = dw_value.is_some();
        let cid_default_width = dw_value.unwrap_or(1000.0);

        let resolved_cidfont_dict = if let Some(w_obj) = cidfont_dict.get("W") {
            if let Some(r) = w_obj.as_reference() {
                match doc.load_object(r) {
                    Ok(resolved) => {
                        let mut dict_clone = cidfont_dict.clone();
                        dict_clone.insert("W".to_string(), resolved);
                        std::borrow::Cow::Owned(dict_clone)
                    }
                    Err(error) => {
                        crate::error::trace_recovery("resolve_cid_widths", &error);
                        std::borrow::Cow::Borrowed(cidfont_dict)
                    }
                }
            } else {
                std::borrow::Cow::Borrowed(cidfont_dict)
            }
        } else {
            std::borrow::Cow::Borrowed(cidfont_dict)
        };
        let cid_widths = Self::parse_cid_widths(&resolved_cidfont_dict, base_font);

        if cid_widths.is_some() {
            tracing::debug!(
                "Font '{}': Parsed CID widths - {} entries, default width {}",
                base_font,
                cid_widths.as_ref().map(|m| m.len()).unwrap_or(0),
                cid_default_width
            );
        }

        let resolved_for_w2 = if let Some(w2_obj) = cidfont_dict.get("W2") {
            if let Some(r) = w2_obj.as_reference() {
                match doc.load_object(r) {
                    Ok(resolved) => {
                        let mut dict_clone = resolved_cidfont_dict.clone().into_owned();
                        dict_clone.insert("W2".to_string(), resolved);
                        std::borrow::Cow::Owned(dict_clone)
                    }
                    Err(error) => {
                        crate::error::trace_recovery("resolve_cid_vertical_metrics", &error);
                        resolved_cidfont_dict.clone()
                    }
                }
            } else {
                resolved_cidfont_dict.clone()
            }
        } else {
            resolved_cidfont_dict.clone()
        };
        let cid_vertical_metrics = Self::parse_cid_vertical_metrics(&resolved_for_w2, base_font);
        let cid_default_vertical_metrics = Self::parse_dw2(&resolved_for_w2);
        if cid_vertical_metrics.is_some() {
            tracing::debug!(
                "Font '{}': Parsed /W2 vertical metrics - {} entries, /DW2 defaults w1y={} v_x={} v_y={}",
                base_font,
                cid_vertical_metrics.as_ref().map(|m| m.len()).unwrap_or(0),
                cid_default_vertical_metrics.w1y,
                cid_default_vertical_metrics.v_x,
                cid_default_vertical_metrics.v_y,
            );
        }

        let descendant_tt_cmap = if cid_font_type == "CIDFontType2" {
            Self::extract_truetype_cmap_from_descriptor(cidfont_dict, base_font, doc)
        } else {
            None
        };

        let descendant_embedded = Self::extract_embedded_font_from_descriptor(cidfont_dict, base_font, doc);

        let (desc_raw_ascent, desc_raw_descent) = Self::read_raw_ascent_descent_from_descriptor(cidfont_dict, doc);

        Ok((
            cid_to_gid_map,
            cid_system_info,
            Some(cid_font_type),
            cid_widths,
            cid_default_width,
            has_explicit_dw,
            descendant_tt_cmap,
            descendant_embedded,
            desc_raw_ascent,
            desc_raw_descent,
            cid_vertical_metrics,
            cid_default_vertical_metrics,
        ))
    }

    /// Resolves `/CIDToGIDMap` for a descendant CIDFont, moved verbatim out of
    /// [`Self::parse_descendant_fonts`]. `None` for CIDFontType0 (CFF/OpenType has no
    /// GID indirection); every failure path for CIDFontType2 (missing key, non-Identity
    /// name, non-name/non-reference value, unreadable stream, malformed stream length,
    /// empty stream) falls back to `CIDToGIDMap::Identity`, exactly as the inline
    /// version did. ~keep
    fn resolve_cid_to_gid_map(
        cidfont_dict: &HashMap<String, Object>,
        cid_font_type: &str,
        base_font: &str,
        doc: &PdfDocument,
    ) -> Option<CIDToGIDMap> {
        if cid_font_type != "CIDFontType2" {
            tracing::debug!(
                "Font '{}': CIDFontType0 (CFF/OpenType) - no CIDToGIDMap needed",
                base_font
            );
            return None;
        }

        let Some(cidtogid_obj) = cidfont_dict.get("CIDToGIDMap") else {
            tracing::debug!(
                "Font '{}': CIDToGIDMap not specified, defaulting to Identity",
                base_font
            );
            return Some(CIDToGIDMap::Identity);
        };

        if let Some(name) = cidtogid_obj.as_name() {
            if name == "Identity" {
                tracing::debug!("Font '{}': CIDToGIDMap is Identity", base_font);
            } else {
                tracing::warn!(
                    target: crate::LOG_TARGET_ROOT,
                    operation = "parse_cid_to_gid_map",
                    error_code = "invalid_map_name",
                    "using identity CID-to-GID map"
                );
            }
            return Some(CIDToGIDMap::Identity);
        }

        let Some(stream_ref) = cidtogid_obj.as_reference() else {
            tracing::warn!(
                target: crate::LOG_TARGET_ROOT,
                operation = "parse_cid_to_gid_map",
                error_code = "invalid_map_type",
                "using identity CID-to-GID map"
            );
            return Some(CIDToGIDMap::Identity);
        };

        let stream_data = doc
            .load_object(stream_ref)
            .inspect_err(|error| crate::error::trace_recovery("load_cid_to_gid_map", error))
            .ok()
            .and_then(|stream_obj| {
                doc.decode_stream_with_encryption(&stream_obj, stream_ref)
                    .inspect_err(|error| crate::error::trace_recovery("decode_cid_to_gid_map", error))
                    .ok()
            });

        let Some(stream_data) = stream_data else {
            return Some(CIDToGIDMap::Identity);
        };

        if stream_data.len() % 2 != 0 {
            tracing::warn!(
                target: crate::LOG_TARGET_ROOT,
                operation = "parse_cid_to_gid_map",
                error_code = "odd_stream_length",
                byte_count = stream_data.len(),
                "using identity CID-to-GID map"
            );
            return Some(CIDToGIDMap::Identity);
        }
        if stream_data.is_empty() {
            tracing::warn!(
                target: crate::LOG_TARGET_ROOT,
                operation = "parse_cid_to_gid_map",
                error_code = "empty_stream",
                byte_count = 0usize,
                "using identity CID-to-GID map"
            );
            return Some(CIDToGIDMap::Identity);
        }

        let num_entries = stream_data.len() / 2;
        let mut map = Vec::with_capacity(num_entries);
        for i in 0..num_entries {
            let gid = u16::from_be_bytes([stream_data[i * 2], stream_data[i * 2 + 1]]);
            map.push(gid);
        }
        tracing::debug!(
            "Font '{}': Loaded explicit CIDToGIDMap with {} entries",
            base_font,
            num_entries
        );
        Some(CIDToGIDMap::Explicit(map))
    }

    /// Extract TrueType cmap from a font dictionary's /FontDescriptor /FontFile2.
    fn extract_truetype_cmap_from_descriptor(
        font_dict: &HashMap<String, Object>,
        base_font: &str,
        doc: &PdfDocument,
    ) -> Option<TrueTypeCMap> {
        let desc_obj = font_dict.get("FontDescriptor")?;
        let desc = if let Some(r) = desc_obj.as_reference() {
            doc.load_object(r).ok()?
        } else {
            desc_obj.clone()
        };
        let desc_dict = desc.as_dict()?;
        let ff2_obj = desc_dict.get("FontFile2")?;
        let ff2_ref = ff2_obj.as_reference()?;
        let ff2_stream = match doc.load_object(ff2_ref) {
            Ok(obj) => obj,
            Err(error) => {
                crate::error::trace_recovery("load_truetype_font_program", &error);
                return None;
            }
        };
        let font_data = match doc.decode_stream_with_encryption(&ff2_stream, ff2_ref) {
            Ok(data) => data,
            Err(error) => {
                crate::error::trace_recovery("decode_truetype_font_program", &error);
                return None;
            }
        };
        if font_data.is_empty() {
            return None;
        }
        match TrueTypeCMap::from_font_data(&font_data) {
            Ok(cmap) if !cmap.is_empty() => {
                tracing::debug!(
                    "Font '{}': Extracted TrueType cmap from descendant CIDFont ({} mappings)",
                    base_font,
                    cmap.len()
                );
                Some(cmap)
            }
            _ => None,
        }
    }

    /// Read raw /Ascent and /Descent from a font dictionary's /FontDescriptor.
    /// Returns (raw_ascent, raw_descent) in PDF 1/1000-em units, or None if absent.
    /// Used to pull ascent/descent off a CIDFont descendant (§9.7.4 / Table 117).
    fn read_raw_ascent_descent_from_descriptor(
        font_dict: &HashMap<String, Object>,
        doc: &PdfDocument,
    ) -> (Option<f32>, Option<f32>) {
        let desc_obj = match font_dict.get("FontDescriptor") {
            Some(obj) => obj,
            None => return (None, None),
        };
        let desc = if let Some(r) = desc_obj.as_reference() {
            match doc.load_object(r) {
                Ok(obj) => obj,
                Err(_) => return (None, None),
            }
        } else {
            desc_obj.clone()
        };
        let desc_dict = match desc.as_dict() {
            Some(d) => d,
            None => return (None, None),
        };
        let read_f32 = |key: &str| -> Option<f32> {
            desc_dict.get(key).and_then(|o| {
                o.as_real()
                    .map(|r| r as f32)
                    .or_else(|| o.as_integer().map(|i| i as f32))
            })
        };
        (read_f32("Ascent"), read_f32("Descent"))
    }

    /// Extract embedded font data from a font dictionary's /FontDescriptor.
    /// Checks FontFile2 (TrueType), FontFile3 (CFF/OpenType), and FontFile (Type 1).
    fn extract_embedded_font_from_descriptor(
        font_dict: &HashMap<String, Object>,
        base_font: &str,
        doc: &PdfDocument,
    ) -> (bool, Option<Arc<Vec<u8>>>) {
        let Some(desc_obj) = font_dict.get("FontDescriptor") else {
            return (false, None);
        };
        let desc = if let Some(r) = desc_obj.as_reference() {
            match doc.load_object(r) {
                Ok(obj) => obj,
                Err(_) => return (false, None),
            }
        } else {
            desc_obj.clone()
        };
        let Some(desc_dict) = desc.as_dict() else {
            return (false, None);
        };

        let font_file_keys = ["FontFile2", "FontFile3", "FontFile"];
        // Key presence ≠ extraction success: callers gating on "the document
        // embeds its own outlines" must see `true` even when every present
        // stream fails to load/decode below. ~keep
        let has_font_program = font_file_keys.iter().any(|key| desc_dict.contains_key(*key));
        for key in &font_file_keys {
            if let Some(ff_obj) = desc_dict.get(*key) {
                let ff_ref = match ff_obj.as_reference() {
                    Some(r) => r,
                    None => continue,
                };
                let ff_stream = match doc.load_object(ff_ref) {
                    Ok(obj) => obj,
                    Err(error) => {
                        crate::error::trace_recovery("load_embedded_font_program", &error);
                        continue;
                    }
                };
                let font_data = match doc.decode_stream_with_encryption(&ff_stream, ff_ref) {
                    Ok(data) => data,
                    Err(error) => {
                        crate::error::trace_recovery("decode_embedded_font_program", &error);
                        continue;
                    }
                };
                if !font_data.is_empty() {
                    let font_data =
                        if *key == "FontFile3" && !font_data.is_empty() && font_data[0] == 1 && font_data.len() > 4 {
                            tracing::debug!(
                                "Font '{}': Wrapping raw CFF in OpenType container ({} bytes)",
                                base_font,
                                font_data.len()
                            );
                            wrap_cff_in_opentype(&font_data)
                        } else {
                            font_data
                        };
                    tracing::debug!(
                        "Font '{}': Extracted embedded font from {} ({} bytes)",
                        base_font,
                        key,
                        font_data.len()
                    );
                    return (has_font_program, Some(Arc::new(font_data)));
                }
            }
        }
        (has_font_program, None)
    }
}

/// Wrap raw CFF font data in a minimal OpenType container so ttf-parser can parse it.
/// Creates an OpenType font with `head` and `CFF ` tables (both required by ttf-parser).
fn wrap_cff_in_opentype(cff_data: &[u8]) -> Vec<u8> {
    let num_tables: u16 = 4;
    let search_range: u16 = 32;
    let entry_selector: u16 = 2;
    let range_shift: u16 = (num_tables * 16) - search_range;

    let head_table: [u8; 54] = [
        0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x5F, 0x0F, 0x3C, 0xF5, 0x00, 0x0B,
        0x03, 0xE8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0xFF, 0x38, 0xFF, 0x38, 0x03, 0xE8, 0x03, 0xE8, 0x00, 0x00, 0x00, 0x08, 0x00, 0x02, 0x00, 0x01, 0x00, 0x00,
    ];

    let hhea_table: [u8; 36] = [
        0x00, 0x01, 0x00, 0x00, 0x03, 0x20, 0xFF, 0x38, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00,
        0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00,
    ];

    // Minimal maxp table (6 bytes for CFF fonts — version 0.5) ~keep
    let maxp_table: [u8; 6] = [0x00, 0x00, 0x50, 0x00, 0x01, 0x00];

    let header_size: u32 = 12 + (num_tables as u32) * 16;
    // Place tables: head, hhea, maxp, CFF (alphabetical by tag within each group) ~keep
    let head_offset = (header_size + 3) & !3;
    let head_len = head_table.len() as u32;
    let hhea_offset = ((head_offset + head_len) + 3) & !3;
    let hhea_len = hhea_table.len() as u32;
    let maxp_offset = ((hhea_offset + hhea_len) + 3) & !3;
    let maxp_len = maxp_table.len() as u32;
    let cff_offset = ((maxp_offset + maxp_len) + 3) & !3;
    let cff_len = cff_data.len() as u32;

    fn table_checksum(data: &[u8]) -> u32 {
        let mut sum: u32 = 0;
        for chunk in data.chunks(4) {
            let mut bytes = [0u8; 4];
            bytes[..chunk.len()].copy_from_slice(chunk);
            sum = sum.wrapping_add(u32::from_be_bytes(bytes));
        }
        sum
    }

    let mut out = Vec::with_capacity((cff_offset + cff_len) as usize);

    out.extend_from_slice(b"OTTO");
    out.extend_from_slice(&num_tables.to_be_bytes());
    out.extend_from_slice(&search_range.to_be_bytes());
    out.extend_from_slice(&entry_selector.to_be_bytes());
    out.extend_from_slice(&range_shift.to_be_bytes());

    // Table record: CFF (alphabetical order: CFF before head) ~keep
    out.extend_from_slice(b"CFF ");
    out.extend_from_slice(&table_checksum(cff_data).to_be_bytes());
    out.extend_from_slice(&cff_offset.to_be_bytes());
    out.extend_from_slice(&cff_len.to_be_bytes());

    out.extend_from_slice(b"head");
    out.extend_from_slice(&table_checksum(&head_table).to_be_bytes());
    out.extend_from_slice(&head_offset.to_be_bytes());
    out.extend_from_slice(&head_len.to_be_bytes());

    out.extend_from_slice(b"hhea");
    out.extend_from_slice(&table_checksum(&hhea_table).to_be_bytes());
    out.extend_from_slice(&hhea_offset.to_be_bytes());
    out.extend_from_slice(&hhea_len.to_be_bytes());

    out.extend_from_slice(b"maxp");
    out.extend_from_slice(&table_checksum(&maxp_table).to_be_bytes());
    out.extend_from_slice(&maxp_offset.to_be_bytes());
    out.extend_from_slice(&maxp_len.to_be_bytes());

    while out.len() < head_offset as usize {
        out.push(0);
    }
    out.extend_from_slice(&head_table);

    while out.len() < hhea_offset as usize {
        out.push(0);
    }
    out.extend_from_slice(&hhea_table);

    while out.len() < maxp_offset as usize {
        out.push(0);
    }
    out.extend_from_slice(&maxp_table);

    while out.len() < cff_offset as usize {
        out.push(0);
    }

    out.extend_from_slice(cff_data);

    out
}

impl FontInfo {
    /// Parse CIDFont /W array for glyph widths.
    ///
    /// Per PDF Spec ISO 32000-1:2008, Section 9.7.4.3, the /W array has two formats:
    /// - `c [w1 w2 ... wn]` - CID c has width w1, c+1 has width w2, etc.
    /// - `cfirst clast w` - CIDs from cfirst to clast all have width w
    ///
    /// These formats can be mixed in a single array.
    ///
    /// # Example /W array
    /// ```pdf
    /// /W [
    ///   1 [500 600 700] % CID 1=500, CID 2=600, CID 3=700
    ///   100 200 300 % CIDs 100-200 all have width 300
    /// ]
    /// ```
    /// Inspect a Type0 font's `/Encoding` object and resolve the writing
    /// mode it implies, plus the encoding name preserved for diagnostics.
    ///
    /// Returns a pair `(name, wmode)` where:
    /// - `name` is the predefined-CMap name when `/Encoding` is a `/Name`
    ///   atom (`Identity-H`, `Identity-V`, `UniJIS-UTF16-V`, …) or the
    ///   embedded CMap stream's `/CMapName` value when `/Encoding` is a
    ///   stream/dict reference.
    /// - `wmode` is `1` when the resolved name ends in `-V` or equals the
    ///   bare legacy `V`, or when the embedded CMap stream contains a
    ///   `/WMode 1 def` directive. `0` otherwise (including unknown).
    ///
    /// The two signals are surfaced separately so callers can apply the
    /// precedence rules from ISO 32000-1 §9.7.5.4: an embedded CMap stream's
    /// explicit `/WMode` overrides what the name might suggest.
    fn resolve_encoding_writing_mode(enc_obj: &Object, doc: &PdfDocument) -> (Option<String>, u8) {
        if let Some(name) = enc_obj.as_name() {
            let wmode = wmode_from_predefined_cmap_name(name);
            return (Some(name.to_string()), wmode);
        }

        let dict = enc_obj.as_dict();
        let name = dict
            .and_then(|d| d.get("CMapName"))
            .and_then(|n| n.as_name())
            .map(|s| s.to_string());

        // Try to decode the CMap stream and scan for /WMode. We swallow
        // decode errors here — if the stream cannot be decoded, the existing
        // `parse_encoding` path will eventually log it; for wmode detection
        // we silently fall back to the name-based signal. ~keep
        let stream_wmode = match enc_obj.decode_stream_data() {
            Ok(bytes) => {
                let content = String::from_utf8_lossy(&bytes);
                crate::fonts::cmap::parse_wmode_directive_public(&content)
            }
            Err(_) => None,
        };
        let _ = doc;

        let name_wmode = name.as_deref().map(wmode_from_predefined_cmap_name).unwrap_or(0);
        let wmode = stream_wmode.unwrap_or(name_wmode);
        (name, wmode)
    }

    /// Parse `/DW2` from a CIDFont dictionary.
    ///
    /// Per ISO 32000-1 §9.7.4.3 the value is an array of two numbers:
    /// `[v_y_default w1y_default]`. Spec default when `/DW2` is absent is
    /// `[880 -1000]`. The default `v_x` is always `500` (half-em) — the spec
    /// does not provide a way to override it via `/DW2`.
    ///
    /// Returns the parsed defaults, or [`VerticalMetrics::SPEC_DEFAULT`] when
    /// `/DW2` is missing or malformed.
    fn parse_dw2(cidfont_dict: &HashMap<String, Object>) -> VerticalMetrics {
        let Some(dw2_obj) = cidfont_dict.get("DW2") else {
            return VerticalMetrics::SPEC_DEFAULT;
        };
        let Some(arr) = dw2_obj.as_array() else {
            return VerticalMetrics::SPEC_DEFAULT;
        };
        if arr.len() < 2 {
            return VerticalMetrics::SPEC_DEFAULT;
        }
        let v_y = match &arr[0] {
            Object::Integer(i) => *i as f32,
            Object::Real(r) => *r as f32,
            _ => return VerticalMetrics::SPEC_DEFAULT,
        };
        let w1y = match &arr[1] {
            Object::Integer(i) => *i as f32,
            Object::Real(r) => *r as f32,
            _ => return VerticalMetrics::SPEC_DEFAULT,
        };
        VerticalMetrics { w1y, v_x: 500.0, v_y }
    }

    /// Parse `/W2` (per-CID vertical metrics) from a CIDFont dictionary.
    ///
    /// Per ISO 32000-1 §9.7.4.3 the `/W2` array uses two forms, both of which
    /// may be intermixed within a single `/W2`:
    ///
    /// - Form A — explicit per-CID metrics:
    ///   `c [ w1y v_x v_y w1y v_x v_y … ]` — the inner array holds successive
    ///   `(w1y, v_x, v_y)` triples assigned to CIDs `c, c+1, c+2, …`.
    ///
    /// - Form B — range:
    ///   `c_first c_last w1y v_x v_y` — every CID in `c_first..=c_last`
    ///   shares the same `(w1y, v_x, v_y)`.
    ///
    /// Returns `None` when `/W2` is absent or empty, allowing callers to skip
    /// the HashMap allocation entirely on horizontal fonts.
    /// Applies the `/W2` array-form group `c [ w1y v_x v_y … ]` (Form A) for one CID
    /// range. Extracted verbatim from `parse_cid_vertical_metrics`'s `Object::Array`
    /// arm: every `break` in the original body only left the arm's own inner `while`
    /// loop, never the caller's outer loop, so this is pure code motion. ~keep
    fn apply_cid_vertical_metrics_triples(
        cid_start: u16,
        triples: &[Object],
        metrics: &mut HashMap<u16, VerticalMetrics>,
    ) {
        // Walk the inner array in groups of three. A triple is atomic: if any of its
        // three elements is non-numeric we drop the WHOLE triple (advance j+=3,
        // emitted+=1) so the CID alignment of the rest of the inner array is
        // preserved. The original implementation advanced j by 1 on a malformed
        // element, which silently shifted every subsequent CID by one slot. ~keep
        let mut j = 0;
        let mut emitted: u32 = 0;
        let read_num = |obj: &Object| -> Option<f32> {
            match obj {
                Object::Integer(v) => Some(*v as f32),
                Object::Real(v) => Some(*v as f32),
                _ => None,
            }
        };
        while j + 2 < triples.len() {
            let triple = (
                read_num(&triples[j]),
                read_num(&triples[j + 1]),
                read_num(&triples[j + 2]),
            );
            // Compute CID with overflow detection BEFORE writing. saturating_add(emitted)
            // would collapse every overflowing slot onto u16::MAX; instead we stop. ~keep
            let Some(cid) = (cid_start as u32).checked_add(emitted) else {
                tracing::warn!(
                    target: crate::LOG_TARGET_ROOT,
                    operation = "parse_cid_vertical_metrics",
                    error_code = "cid_overflow",
                    start_cid = cid_start,
                    emitted_count = emitted,
                    "stopping vertical metrics parsing"
                );
                break;
            };
            if cid > u16::MAX as u32 {
                tracing::warn!(
                    target: crate::LOG_TARGET_ROOT,
                    operation = "parse_cid_vertical_metrics",
                    error_code = "cid_out_of_range",
                    start_cid = cid_start,
                    emitted_count = emitted,
                    "stopping vertical metrics parsing"
                );
                break;
            }
            match triple {
                (Some(w1y), Some(v_x), Some(v_y)) => {
                    metrics.insert(cid as u16, VerticalMetrics { w1y, v_x, v_y });
                }
                _ => {
                    tracing::warn!(
                        target: crate::LOG_TARGET_ROOT,
                        operation = "parse_cid_vertical_metrics",
                        error_code = "invalid_metric_triple",
                        start_cid = cid_start,
                        emitted_count = emitted,
                        "skipping invalid vertical metric"
                    );
                }
            }
            emitted += 1;
            j += 3;
        }
    }

    fn parse_cid_vertical_metrics(
        cidfont_dict: &HashMap<String, Object>,
        _base_font: &str,
    ) -> Option<HashMap<u16, VerticalMetrics>> {
        let w2_obj = cidfont_dict.get("W2")?;
        let w2_array = w2_obj.as_array()?;

        if w2_array.is_empty() {
            return None;
        }

        let mut metrics: HashMap<u16, VerticalMetrics> = HashMap::new();
        let mut i = 0;

        while i < w2_array.len() {
            let cid_start = match &w2_array[i] {
                Object::Integer(c) => *c as u16,
                _ => {
                    tracing::warn!(
                        target: crate::LOG_TARGET_ROOT,
                        operation = "parse_cid_vertical_metrics",
                        error_code = "invalid_start_cid",
                        entry_index = i,
                        "skipping invalid vertical metrics entry"
                    );
                    i += 1;
                    continue;
                }
            };
            i += 1;

            if i >= w2_array.len() {
                break;
            }

            match &w2_array[i] {
                Object::Array(triples) => {
                    Self::apply_cid_vertical_metrics_triples(cid_start, triples, &mut metrics);
                    i += 1;
                }
                Object::Integer(cid_end_int) => {
                    let cid_end = *cid_end_int as u16;
                    i += 1;
                    if i + 2 >= w2_array.len() {
                        tracing::warn!(
                            target: crate::LOG_TARGET_ROOT,
                            operation = "parse_cid_vertical_metrics",
                            error_code = "truncated_range",
                            start_cid = cid_start,
                            "stopping truncated vertical metrics range"
                        );
                        break;
                    }
                    let read = |obj: &Object| -> Option<f32> {
                        match obj {
                            Object::Integer(v) => Some(*v as f32),
                            Object::Real(v) => Some(*v as f32),
                            _ => None,
                        }
                    };
                    let Some(w1y) = read(&w2_array[i]) else {
                        i += 3;
                        continue;
                    };
                    let Some(v_x) = read(&w2_array[i + 1]) else {
                        i += 3;
                        continue;
                    };
                    let Some(v_y) = read(&w2_array[i + 2]) else {
                        i += 3;
                        continue;
                    };
                    i += 3;
                    let metric = VerticalMetrics { w1y, v_x, v_y };
                    for cid in cid_start..=cid_end {
                        metrics.insert(cid, metric);
                    }
                }
                _ => {
                    tracing::warn!(
                        target: crate::LOG_TARGET_ROOT,
                        operation = "parse_cid_vertical_metrics",
                        error_code = "invalid_range_type",
                        start_cid = cid_start,
                        "skipping invalid vertical metrics range"
                    );
                    i += 1;
                }
            }
        }

        if metrics.is_empty() { None } else { Some(metrics) }
    }

    /// Applies the `/W` array-form width group `c [w1 w2 …]` for one CID range.
    /// Extracted verbatim from `parse_cid_widths`'s `Object::Array` arm — no control
    /// flow crosses into the caller's loop, so this is pure code motion. ~keep
    fn apply_cid_width_array(cid_start: u16, width_array: &[Object], widths: &mut HashMap<u16, f32>) {
        for (j, width_obj) in width_array.iter().enumerate() {
            let width = match width_obj {
                Object::Integer(w) => *w as f32,
                Object::Real(w) => *w as f32,
                _ => continue,
            };
            let cid = cid_start.saturating_add(j as u16);
            widths.insert(cid, width);
        }
    }

    fn parse_cid_widths(cidfont_dict: &HashMap<String, Object>, _base_font: &str) -> Option<HashMap<u16, f32>> {
        let w_obj = cidfont_dict.get("W")?;
        let w_array = w_obj.as_array()?;

        if w_array.is_empty() {
            return None;
        }

        let mut widths: HashMap<u16, f32> = HashMap::new();
        let mut i = 0;

        while i < w_array.len() {
            let cid_start = match &w_array[i] {
                Object::Integer(c) => *c as u16,
                _ => {
                    tracing::warn!(
                        target: crate::LOG_TARGET_ROOT,
                        operation = "parse_cid_widths",
                        error_code = "invalid_start_cid",
                        entry_index = i,
                        "skipping invalid width entry"
                    );
                    i += 1;
                    continue;
                }
            };
            i += 1;

            if i >= w_array.len() {
                break;
            }

            // Second element is either:
            // - An array of widths (format: c [w1 w2 ...])
            // - An integer CID end (format: cfirst clast w) ~keep
            match &w_array[i] {
                Object::Array(width_array) => {
                    Self::apply_cid_width_array(cid_start, width_array, &mut widths);
                    i += 1;
                }
                Object::Integer(cid_end) => {
                    let cid_end = *cid_end as u16;
                    i += 1;

                    if i >= w_array.len() {
                        tracing::warn!(
                            target: crate::LOG_TARGET_ROOT,
                            operation = "parse_cid_widths",
                            error_code = "missing_range_width",
                            start_cid = cid_start,
                            end_cid = cid_end,
                            "stopping truncated width range"
                        );
                        break;
                    }

                    let width = match &w_array[i] {
                        Object::Integer(w) => *w as f32,
                        Object::Real(w) => *w as f32,
                        _ => {
                            tracing::warn!(
                                target: crate::LOG_TARGET_ROOT,
                                operation = "parse_cid_widths",
                                error_code = "invalid_range_width",
                                start_cid = cid_start,
                                end_cid = cid_end,
                                "skipping invalid width range"
                            );
                            i += 1;
                            continue;
                        }
                    };
                    i += 1;

                    for cid in cid_start..=cid_end {
                        widths.insert(cid, width);
                    }
                }
                _ => {
                    tracing::warn!(
                        target: crate::LOG_TARGET_ROOT,
                        operation = "parse_cid_widths",
                        error_code = "invalid_range_type",
                        start_cid = cid_start,
                        "skipping invalid width range"
                    );
                    i += 1;
                }
            }
        }

        if widths.is_empty() { None } else { Some(widths) }
    }

    /// Vertical advance and origin offset for a CID, in 1000ths-of-em.
    ///
    /// Lookup order:
    /// 1. Per-CID entry from `/W2` (if `cid_vertical_metrics` is populated).
    /// 2. `/DW2` defaults (`cid_default_vertical_metrics`).
    /// 3. Spec defaults from [`VerticalMetrics::SPEC_DEFAULT`] when the font
    ///    is not a CIDFont (e.g. simple Type1/TrueType): callers that
    ///    reach this with a non-Type0 font are degenerate, but returning
    ///    spec defaults is safe.
    ///
    /// This is the vertical counterpart to [`FontInfo::get_glyph_width`] and
    /// is read on the hot path of the renderer / extractor whenever
    /// `self.wmode == 1`.
    #[inline]
    pub fn get_vertical_metrics(&self, cid: u16) -> VerticalMetrics {
        if let Some(map) = &self.cid_vertical_metrics
            && let Some(&m) = map.get(&cid)
        {
            return m;
        }
        self.cid_default_vertical_metrics
    }

    /// If `dict` (an /Encoding dictionary) carries a /CMapName, resolves the Type0
    /// CMap-stream encoding case and returns `Some` of the result to return early;
    /// returns `None` when there is no /CMapName, telling the caller to continue with
    /// the /BaseEncoding + /Differences path. Extracted verbatim from `parse_encoding`
    /// — a pure early-return block, so wrapping it in `Option` preserves the original
    /// `return Ok(...)` behavior exactly. ~keep
    fn try_parse_cmap_stream_encoding(
        dict: &HashMap<String, Object>,
    ) -> Option<Result<(Encoding, HashMap<u8, String>, HashMap<u8, String>)>> {
        let cmap_name = dict.get("CMapName").and_then(|n| n.as_name())?;

        let is_adobe_collection = cmap_name.starts_with("Adobe-")
            && (cmap_name.contains("Japan")
                || cmap_name.contains("GB")
                || cmap_name.contains("CNS")
                || cmap_name.contains("Korea"));
        if is_adobe_collection {
            tracing::debug!(
                "Encoding is Adobe CMap stream (CMapName={:?}), treating as Identity",
                cmap_name
            );
            return Some(Ok((Encoding::Identity, HashMap::new(), HashMap::new())));
        }
        if cmap_name == "Identity-H" || cmap_name == "Identity-V" {
            return Some(Ok((Encoding::Identity, HashMap::new(), HashMap::new())));
        }
        tracing::debug!(
            "Encoding is custom CMap stream (CMapName={:?}), treating as Standard",
            cmap_name
        );
        Some(Ok((
            Encoding::Standard(cmap_name.to_string()),
            HashMap::new(),
            HashMap::new(),
        )))
    }

    /// Resolves the base `code → char` map an /Encoding dictionary's /Differences
    /// array (if any) is layered on top of: an explicit /BaseEncoding name, else the
    /// font program's built-in encoding (PDF Spec ISO 32000-1:2008 §9.6.6.1), else
    /// plain StandardEncoding. Extracted verbatim from `parse_encoding`. ~keep
    fn resolve_base_encoding_map(
        dict: &HashMap<String, Object>,
        doc: &PdfDocument,
        font_program_encoding: Option<&HashMap<u8, char>>,
    ) -> HashMap<u8, char> {
        if let Some(base_enc_obj) = dict.get("BaseEncoding") {
            let resolved_base = if let Some(obj_ref) = base_enc_obj.as_reference() {
                doc.load_object(obj_ref).ok()
            } else {
                None
            };
            let base_obj = resolved_base.as_ref().unwrap_or(base_enc_obj);

            let Some(base_name) = base_obj.as_name() else {
                return HashMap::new();
            };
            let mut map = HashMap::new();
            for code in 0u8..=255 {
                let Some(unicode_str) = standard_encoding_lookup(base_name, code) else {
                    continue;
                };
                let Some(ch) = unicode_str.chars().next() else {
                    continue;
                };
                map.insert(code, ch);
            }
            map
        } else if let Some(prog_enc) = font_program_encoding {
            // PDF Spec ISO 32000-1:2008, Section 9.6.6.1:
            // "If BaseEncoding is absent and the font has a built-in encoding,
            // the built-in encoding shall be used as the base encoding." ~keep
            prog_enc.clone()
        } else {
            let mut map = HashMap::new();
            for code in 0u8..=255 {
                let Some(unicode_str) = standard_encoding_lookup("StandardEncoding", code) else {
                    continue;
                };
                let Some(ch) = unicode_str.chars().next() else {
                    continue;
                };
                map.insert(code, ch);
            }
            map
        }
    }

    /// Applies an /Encoding dictionary's `/Differences` array (if present) on top of
    /// `encoding_map`, filling in `multi_char_map` (compound glyph names) and
    /// `diff_glyph_names` (the raw glyph name per code, for downstream punctuation
    /// recovery) as it goes. A no-op when `/Differences` is absent. Extracted verbatim
    /// from `parse_encoding`; no control flow crosses into the caller. ~keep
    fn apply_differences_array(
        dict: &HashMap<String, Object>,
        doc: &PdfDocument,
        encoding_map: &mut HashMap<u8, char>,
        multi_char_map: &mut HashMap<u8, String>,
        diff_glyph_names: &mut HashMap<u8, String>,
    ) {
        let Some(differences_obj) = dict.get("Differences") else {
            return;
        };
        tracing::debug!("Found /Differences array in encoding dictionary");

        let resolved_diff = if let Some(obj_ref) = differences_obj.as_reference() {
            doc.load_object(obj_ref).ok()
        } else {
            None
        };
        let diff_obj = resolved_diff.as_ref().unwrap_or(differences_obj);

        let Some(diff_array) = diff_obj.as_array() else {
            tracing::warn!(
                target: crate::LOG_TARGET_ROOT,
                operation = "parse_font_encoding",
                error_code = "invalid_differences_type",
                "ignoring malformed font differences"
            );
            return;
        };

        tracing::debug!("/Differences array has {} items", diff_array.len());
        let mut current_code: u32 = 0;

        for item in diff_array {
            let resolved_item = if let Some(obj_ref) = item.as_reference() {
                doc.load_object(obj_ref).ok()
            } else {
                None
            };
            let actual_item = resolved_item.as_ref().unwrap_or(item);

            match actual_item {
                Object::Integer(code) => {
                    current_code = *code as u32;
                }
                Object::Name(glyph_name) => {
                    // Retain the authoritative glyph name for this code
                    // (ISO 32000-1 §9.6.6.1, Table 114). Kept regardless
                    // of whether it resolves to a single/compound/unknown
                    // Unicode value, so the punctuation-recovery
                    // interceptions in `char_to_unicode` can consult it. ~keep
                    if current_code <= 255 {
                        diff_glyph_names.insert(current_code as u8, glyph_name.clone());
                    }
                    if let Some(unicode_char) = glyph_name_to_unicode(glyph_name) {
                        if current_code <= 255 {
                            encoding_map.insert(current_code as u8, unicode_char);
                            if is_ligature_char(unicode_char) {
                                tracing::debug!(
                                    "/Differences: code {} → /{} → '{}' (U+{:04X})",
                                    current_code,
                                    glyph_name,
                                    unicode_char,
                                    unicode_char as u32
                                );
                            }
                        }
                    } else if let Some(unicode_string) = glyph_name_to_unicode_string(glyph_name) {
                        // Compound glyph name (e.g. f_f → "ff", f_f_i → "ffi")
                        // ~keep
                        if current_code <= 255 {
                            multi_char_map.insert(current_code as u8, unicode_string.clone());
                            tracing::debug!(
                                "/Differences: code {} → /{} → {:?} (compound)",
                                current_code,
                                glyph_name,
                                unicode_string
                            );
                        }
                    } else {
                        tracing::debug!(
                            "Unknown glyph name '{}' at code {} in /Differences array",
                            glyph_name,
                            current_code
                        );
                    }
                    current_code += 1;
                }
                _ => {
                    tracing::warn!(
                        target: crate::LOG_TARGET_ROOT,
                        operation = "parse_font_encoding",
                        error_code = "invalid_differences_entry",
                        "skipping malformed font encoding entry"
                    );
                }
            }
        }

        tracing::debug!("Parsed /Differences array with {} custom mappings", encoding_map.len());
    }

    /// Handles both named encodings (e.g., /WinAnsiEncoding) and encoding dictionaries
    /// with /Differences arrays that override specific character codes.
    ///
    /// # PDF Spec Reference
    ///
    /// ISO 32000-1:2008, Section 9.6.6.2 - Character Encoding
    ///
    /// A /Differences array has the format:
    /// ```pdf
    /// /Encoding <<
    ///     /BaseEncoding /WinAnsiEncoding
    ///     /Differences [code1 /name1 /name2 ... codeN /nameN ...]
    /// >>
    /// ```
    ///
    /// Where integers specify starting codes, and names specify glyphs for consecutive codes.
    ///
    /// The third element of the returned tuple is the `diff_glyph_names` side
    /// map: `code → /Differences glyph name` for simple fonts (empty otherwise).
    /// It retains the authoritative glyph *name* (not the resolved char) so the
    /// punctuation-recovery interceptions in `char_to_unicode` can consult it.
    fn parse_encoding(
        enc_obj: &Object,
        doc: &PdfDocument,
        font_program_encoding: Option<&HashMap<u8, char>>,
    ) -> Result<(Encoding, HashMap<u8, String>, HashMap<u8, String>)> {
        let empty_map = HashMap::new();
        if let Some(name) = enc_obj.as_name() {
            // Standard encoding names (no /Differences ⇒ no glyph-name side map) ~keep
            match name {
                "WinAnsiEncoding" => Ok((
                    Encoding::Standard("WinAnsiEncoding".to_string()),
                    empty_map,
                    HashMap::new(),
                )),
                "MacRomanEncoding" => Ok((
                    Encoding::Standard("MacRomanEncoding".to_string()),
                    empty_map,
                    HashMap::new(),
                )),
                "MacExpertEncoding" => Ok((
                    Encoding::Standard("MacExpertEncoding".to_string()),
                    empty_map,
                    HashMap::new(),
                )),
                "Identity-H" | "Identity-V" => Ok((Encoding::Identity, empty_map, HashMap::new())),
                _ => Ok((Encoding::Standard(name.to_string()), empty_map, HashMap::new())),
            }
        } else if let Some(dict) = enc_obj.as_dict() {
            // Check if this is a CMap stream (Type0 font encoding reference)
            // Per PDF Spec §9.7.5.2, Type0 fonts can reference a CMap stream
            // via /Encoding. For known Adobe character collections (Japan1, GB1,
            // CNS1, Korea1), these define charcode→CID identity mappings and we
            // can resolve CIDs via predefined CID-to-Unicode tables.
            // For custom CMaps (e.g., "Prince-ArialMT-H"), we preserve the default
            // behavior since we can't parse arbitrary CMap programs yet. ~keep
            if let Some(result) = Self::try_parse_cmap_stream_encoding(dict) {
                return result;
            }

            let mut multi_char_map: HashMap<u8, String> = HashMap::new();
            let mut diff_glyph_names: HashMap<u8, String> = HashMap::new();

            let mut encoding_map: HashMap<u8, char> = Self::resolve_base_encoding_map(dict, doc, font_program_encoding);

            Self::apply_differences_array(dict, doc, &mut encoding_map, &mut multi_char_map, &mut diff_glyph_names);

            if !encoding_map.is_empty() || !multi_char_map.is_empty() {
                Ok((Encoding::Custom(encoding_map), multi_char_map, diff_glyph_names))
            } else {
                Ok((
                    Encoding::Standard("StandardEncoding".to_string()),
                    HashMap::new(),
                    diff_glyph_names,
                ))
            }
        } else {
            Ok((
                Encoding::Standard("StandardEncoding".to_string()),
                HashMap::new(),
                HashMap::new(),
            ))
        }
    }

    /// Map a character code to a Unicode string.
    ///
    /// Priority:
    /// 1. ToUnicode CMap (most accurate)
    /// 2. Built-in encoding
    /// 3. Symbol font encoding (for Symbol/ZapfDingbats fonts)
    /// 4. Ligature expansion (for ligature characters)
    /// 5. Identity mapping (as fallback)
    ///
    /// # Arguments
    ///
    /// * `char_code` - The character code from the PDF content stream
    ///
    /// # Returns
    ///
    /// The Unicode string for this character, or None if no mapping exists.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use xberg_native_pdf::fonts::FontInfo;
    /// # fn example(font: &FontInfo) {
    /// if let Some(unicode) = font.char_to_unicode(0x41) {
    ///     println!("Character: {}", unicode); // Should print "A"
    /// }
    /// # }
    /// ```
    /// Convert a character code to Unicode string.
    ///
    /// Per PDF Spec ISO 32000-1:2008, Section 9.10.2 "Mapping Character Codes to Unicode Values":
    ///
    /// Priority order (STRICTLY FOLLOWED):
    /// 1. ToUnicode CMap (if present) - highest priority, NO EXCEPTIONS
    /// 2. Predefined encodings for simple fonts with standard glyphs
    /// 3. Font descriptor's symbolic flag + built-in encoding (e.g., Symbol, ZapfDingbats)
    /// 4. Font's /Encoding + /Differences
    ///
    /// IMPORTANT: We do NOT apply heuristics to override ToUnicode. If the PDF has
    /// a buggy ToUnicode CMap, that is a PDF authoring error, not our responsibility
    /// to "fix" by guessing what the author meant.
    /// Get glyph width for a character code.
    ///
    /// Returns width in 1000ths of em (PDF units) per PDF Spec ISO 32000-1:2008, Section 9.7.4.
    /// Must be multiplied by (font_size / 1000) to get actual width in user space units.
    ///
    /// # Arguments
    ///
    /// * `char_code` - Character code from PDF content stream (e.g., byte value from Tj/TJ operator)
    ///
    /// # Returns
    ///
    /// Width in 1000ths of em. Returns `default_width` if the character code is not
    /// in the widths array or if widths are not available for this font.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use xberg_native_pdf::fonts::FontInfo;
    ///
    /// # fn example(font: &FontInfo) {
    /// // Get width for character 'A' (code 65)
    /// let width = font.get_glyph_width(65);
    /// let font_size = 12.0;
    /// let actual_width = width * font_size / 1000.0;
    /// println!("Width of 'A' at 12pt: {:.2}pt", actual_width);
    /// # }
    /// ```
    pub fn get_glyph_width(&self, char_code: u16) -> f32 {
        // For Type0 (CID) fonts, use /W array then fall back to /DW (cid_default_width).
        // F15 fix: when /DW was NOT explicitly set (has_explicit_dw=false) and the char
        // code has no entry in /W, fall through to default_width instead of returning
        // the spec-default 1000.
        // NOTE: ISO 32000-1 §9.7.4 Table 117 specifies the default for a missing /DW
        // as 1000 units. This implementation intentionally deviates from that default
        // because many non-fullwidth CID fonts omit /DW; returning 1000 for their glyphs
        // over-estimates widths and disables the gap-correction heuristic. Purely
        // fullwidth CJK fonts that omit /DW may have glyph widths under-estimated as
        // a consequence — an acceptable trade-off for the common mixed-script case. ~keep
        if self.subtype == "Type0" {
            if let Some(cid_widths) = &self.cid_widths
                && let Some(&width) = cid_widths.get(&char_code)
            {
                return width;
            }
            if self.has_explicit_dw {
                return self.cid_default_width;
            }
            // Fall through to default_width — same path as simple fonts without /Widths. ~keep
        }

        if let Some(widths) = &self.widths
            && let Some(first_char) = self.first_char
        {
            let index = char_code as i32 - first_char as i32;
            if index >= 0 && (index as usize) < widths.len() {
                return widths[index as usize];
            }
        }
        if let Some(w) = self.get_standard_font_width(char_code) {
            return w;
        }
        self.default_width
    }

    /// Look up width from standard 14 font metrics when /Widths array is absent
    /// or the char code falls outside the [FirstChar, LastChar] range.
    fn get_standard_font_width(&self, char_code: u16) -> Option<f32> {
        // If a /Widths array covers this specific char code, trust it — don't override
        // with standard metrics. For chars OUTSIDE the range (including the common case
        // where space U+0020 = 32 is below a FirstChar like 66) we prefer named-font
        // metrics over the generic default_width (500), which is often too wide. ~keep
        if let Some(widths) = &self.widths
            && let Some(first_char) = self.first_char
        {
            let index = char_code as i32 - first_char as i32;
            if index >= 0 && (index as usize) < widths.len() {
                return None;
            }
        }
        self.get_standard_font_intrinsic_width(char_code)
    }

    fn get_standard_font_intrinsic_width(&self, char_code: u16) -> Option<f32> {
        // The name classification below is a pure function of `base_font`, but
        // this runs once per glyph — so it is resolved once and memoized. ~keep
        let std14 = (*self.std14_memo.get_or_init(|| self.classify_std14()))?;
        let is_bold = std14.is_bold;
        if std14.is_courier {
            return Some(600.0);
        }
        let is_times = std14.is_times;
        let code = char_code as u8;
        self.std14_width(std14, is_times, is_bold, code)
    }

    /// Classify `base_font` against the Standard-14 set (ISO 32000-1 Annex D).
    /// `None` when the font is not one of the width-bearing standard families.
    fn classify_std14(&self) -> Option<Std14Flags> {
        // F13 fix: use exact match against the canonical 14 standard PDF font names
        // after stripping any SUBSET+ prefix (e.g. "ABCDEF+Helvetica" → "Helvetica").
        // `contains` would incorrectly match "HelveticaCorp-Custom" as Helvetica. ~keep
        let raw_name = &self.base_font;
        let name: &str = if let Some(idx) = raw_name.find('+') {
            // Strip subset prefix: the part after '+' is the actual font name ~keep
            let suffix = &raw_name[idx + 1..];
            if suffix.is_empty() { raw_name } else { suffix }
        } else {
            raw_name
        };
        // Canonical Standard-14 font names per ISO 32000-1 Annex D.
        // "Helvetica-Oblique" is the name used by virtually all real-world PDFs;
        // the spec's canonical PostScript name is "HelveticaOblique" (no hyphen).
        // Both are accepted. ~keep
        const STANDARD_14: &[&str] = &[
            "Courier",
            "Courier-Bold",
            "Courier-BoldOblique",
            "Courier-Oblique",
            "Helvetica",
            "Helvetica-Bold",
            "Helvetica-BoldOblique",
            "Helvetica-Oblique",
            "HelveticaOblique",
            "Times-Roman",
            "Times-Bold",
            "Times-BoldItalic",
            "Times-Italic",
            "Symbol",
            "ZapfDingbats",
        ];
        if !STANDARD_14.contains(&name) {
            return None;
        }
        let is_times = name.starts_with("Times");
        let is_helvetica = name.starts_with("Helvetica");
        let is_courier = name.starts_with("Courier");

        if !is_times && !is_helvetica && !is_courier {
            return None;
        }

        Some(Std14Flags {
            is_times,
            is_courier,
            is_bold: name.contains("Bold"),
            is_bold_italic: name.contains("BoldItalic"),
            is_helvetica,
            is_italic: name.contains("Italic"),
        })
    }

    /// Standard-14 width tables, keyed off the memoized classification.
    fn std14_width(&self, std14: Std14Flags, is_times: bool, is_bold: bool, code: u8) -> Option<f32> {
        // Times-Roman / Times-Bold / Times-BoldItalic standard widths (Adobe AFM metrics) ~keep
        if is_times {
            if std14.is_bold_italic {
                // Times-BoldItalic widths (Adobe Core 14 Fonts AFM). ~keep
                return Some(match code {
                    32 => 250.0,
                    33 => 389.0,
                    34 => 555.0,
                    35 => 500.0,
                    36 => 500.0,
                    37 => 833.0,
                    38 => 778.0,
                    39 => 333.0,
                    40 => 333.0,
                    41 => 333.0,
                    42 => 500.0,
                    43 => 570.0,
                    44 => 250.0,
                    45 => 333.0,
                    46 => 250.0,
                    47 => 278.0,
                    48..=57 => 500.0,
                    58 => 333.0,
                    59 => 333.0,
                    60 => 570.0,
                    61 => 570.0,
                    62 => 570.0,
                    63 => 500.0,
                    64 => 832.0,
                    65 => 667.0,
                    66 => 667.0,
                    67 => 667.0,
                    68 => 722.0,
                    69 => 667.0,
                    70 => 667.0,
                    71 => 722.0,
                    72 => 778.0,
                    73 => 389.0,
                    74 => 500.0,
                    75 => 667.0,
                    76 => 611.0,
                    77 => 889.0,
                    78 => 722.0,
                    79 => 722.0,
                    80 => 611.0,
                    81 => 722.0,
                    82 => 667.0,
                    83 => 556.0,
                    84 => 611.0,
                    85 => 722.0,
                    86 => 667.0,
                    87 => 889.0,
                    88 => 667.0,
                    89 => 611.0,
                    90 => 611.0,
                    91 => 333.0,
                    92 => 278.0,
                    93 => 333.0,
                    94 => 570.0,
                    95 => 500.0,
                    97 => 500.0,
                    98 => 500.0,
                    99 => 444.0,
                    100 => 500.0,
                    101 => 444.0,
                    102 => 333.0,
                    103 => 500.0,
                    104 => 556.0,
                    105 => 278.0,
                    106 => 278.0,
                    107 => 500.0,
                    108 => 278.0,
                    109 => 778.0,
                    110 => 556.0,
                    111 => 500.0,
                    112 => 500.0,
                    113 => 500.0,
                    114 => 389.0,
                    115 => 389.0,
                    116 => 278.0,
                    117 => 556.0,
                    118 => 444.0,
                    119 => 667.0,
                    120 => 500.0,
                    121 => 444.0,
                    122 => 389.0,
                    _ => return None,
                });
            }
            if is_bold {
                // Times-Bold widths (Adobe Core 14 Fonts AFM). ~keep
                return Some(match code {
                    32 => 250.0,
                    33 => 333.0,
                    34 => 555.0,
                    35 => 500.0,
                    36 => 500.0,
                    37 => 1000.0,
                    38 => 833.0,
                    39 => 333.0,
                    40 => 333.0,
                    41 => 333.0,
                    42 => 500.0,
                    43 => 570.0,
                    44 => 250.0,
                    45 => 333.0,
                    46 => 250.0,
                    47 => 278.0,
                    48..=57 => 500.0,
                    58 => 333.0,
                    59 => 333.0,
                    60 => 570.0,
                    61 => 570.0,
                    62 => 570.0,
                    63 => 500.0,
                    64 => 930.0,
                    65 => 722.0,
                    66 => 667.0,
                    67 => 722.0,
                    68 => 722.0,
                    69 => 667.0,
                    70 => 611.0,
                    71 => 778.0,
                    72 => 778.0,
                    73 => 389.0,
                    74 => 500.0,
                    75 => 778.0,
                    76 => 667.0,
                    77 => 944.0,
                    78 => 722.0,
                    79 => 778.0,
                    80 => 611.0,
                    81 => 778.0,
                    82 => 722.0,
                    83 => 556.0,
                    84 => 667.0,
                    85 => 722.0,
                    86 => 722.0,
                    87 => 1000.0,
                    88 => 722.0,
                    89 => 722.0,
                    90 => 667.0,
                    91 => 333.0,
                    92 => 278.0,
                    93 => 333.0,
                    94 => 581.0,
                    95 => 500.0,
                    97 => 500.0,
                    98 => 556.0,
                    99 => 444.0,
                    100 => 556.0,
                    101 => 444.0,
                    102 => 333.0,
                    103 => 500.0,
                    104 => 556.0,
                    105 => 278.0,
                    106 => 333.0,
                    107 => 556.0,
                    108 => 278.0,
                    109 => 833.0,
                    110 => 556.0,
                    111 => 500.0,
                    112 => 556.0,
                    113 => 556.0,
                    114 => 444.0,
                    115 => 389.0,
                    116 => 333.0,
                    117 => 556.0,
                    118 => 500.0,
                    119 => 722.0,
                    120 => 500.0,
                    121 => 500.0,
                    122 => 444.0,
                    _ => return None,
                });
            }
            if std14.is_italic {
                // Times-Italic widths (Adobe Core 14 Fonts AFM). ~keep
                return Some(match code {
                    32 => 250.0,
                    33 => 333.0,
                    34 => 420.0,
                    35 => 500.0,
                    36 => 500.0,
                    37 => 833.0,
                    38 => 778.0,
                    39 => 333.0,
                    40 => 333.0,
                    41 => 333.0,
                    42 => 500.0,
                    43 => 675.0,
                    44 => 250.0,
                    45 => 333.0,
                    46 => 250.0,
                    47 => 278.0,
                    48..=57 => 500.0,
                    58 => 333.0,
                    59 => 333.0,
                    60 => 675.0,
                    61 => 675.0,
                    62 => 675.0,
                    63 => 500.0,
                    64 => 920.0,
                    65 => 611.0,
                    66 => 611.0,
                    67 => 667.0,
                    68 => 722.0,
                    69 => 611.0,
                    70 => 611.0,
                    71 => 722.0,
                    72 => 722.0,
                    73 => 333.0,
                    74 => 444.0,
                    75 => 667.0,
                    76 => 556.0,
                    77 => 833.0,
                    78 => 667.0,
                    79 => 722.0,
                    80 => 611.0,
                    81 => 722.0,
                    82 => 611.0,
                    83 => 500.0,
                    84 => 556.0,
                    85 => 722.0,
                    86 => 611.0,
                    87 => 833.0,
                    88 => 611.0,
                    89 => 556.0,
                    90 => 556.0,
                    91 => 389.0,
                    92 => 278.0,
                    93 => 389.0,
                    94 => 422.0,
                    95 => 500.0,
                    97 => 500.0,
                    98 => 500.0,
                    99 => 444.0,
                    100 => 500.0,
                    101 => 444.0,
                    102 => 278.0,
                    103 => 500.0,
                    104 => 500.0,
                    105 => 278.0,
                    106 => 278.0,
                    107 => 444.0,
                    108 => 278.0,
                    109 => 722.0,
                    110 => 500.0,
                    111 => 500.0,
                    112 => 500.0,
                    113 => 500.0,
                    114 => 389.0,
                    115 => 389.0,
                    116 => 278.0,
                    117 => 500.0,
                    118 => 444.0,
                    119 => 667.0,
                    120 => 444.0,
                    121 => 444.0,
                    122 => 389.0,
                    _ => return None,
                });
            }
            return Some(match code {
                32 => 250.0,
                33 => 333.0,
                34 => 408.0,
                35 => 500.0,
                36 => 500.0,
                37 => 833.0,
                38 => 778.0,
                39 => 333.0,
                40 => 333.0,
                41 => 333.0,
                42 => 500.0,
                43 => 564.0,
                44 => 250.0,
                45 => 333.0,
                46 => 250.0,
                47 => 278.0,
                48 => 500.0,
                49 => 500.0,
                50 => 500.0,
                51 => 500.0,
                52 => 500.0,
                53 => 500.0,
                54 => 500.0,
                55 => 500.0,
                56 => 500.0,
                57 => 500.0,
                58 => 278.0,
                59 => 278.0,
                60 => 564.0,
                61 => 564.0,
                62 => 564.0,
                63 => 444.0,
                64 => 921.0,
                65 => 722.0,
                66 => 667.0,
                67 => 667.0,
                68 => 722.0,
                69 => 611.0,
                70 => 556.0,
                71 => 722.0,
                72 => 722.0,
                73 => 333.0,
                74 => 389.0,
                75 => 722.0,
                76 => 611.0,
                77 => 889.0,
                78 => 722.0,
                79 => 722.0,
                80 => 556.0,
                81 => 722.0,
                82 => 667.0,
                83 => 556.0,
                84 => 611.0,
                85 => 722.0,
                86 => 722.0,
                87 => 944.0,
                88 => 722.0,
                89 => 722.0,
                90 => 611.0,
                91 => 333.0,
                92 => 278.0,
                93 => 333.0,
                97 => 444.0,
                98 => 500.0,
                99 => 444.0,
                100 => 500.0,
                101 => 444.0,
                102 => 333.0,
                103 => 500.0,
                104 => 500.0,
                105 => 278.0,
                106 => 278.0,
                107 => 500.0,
                108 => 278.0,
                109 => 778.0,
                110 => 500.0,
                111 => 500.0,
                112 => 500.0,
                113 => 500.0,
                114 => 333.0,
                115 => 389.0,
                116 => 278.0,
                117 => 500.0,
                118 => 500.0,
                119 => 722.0,
                120 => 500.0,
                121 => 500.0,
                122 => 444.0,
                _ => return None,
            });
        }

        // Helvetica / Helvetica-Bold standard widths (Adobe AFM metrics) ~keep
        if std14.is_helvetica {
            if is_bold {
                // Helvetica-Bold / Helvetica-BoldOblique widths (Adobe Core 14 Fonts AFM). ~keep
                return Some(match code {
                    32 => 278.0,
                    33 => 333.0,
                    34 => 474.0,
                    44 => 278.0,
                    45 => 333.0,
                    46 => 278.0,
                    47 => 278.0,
                    48..=57 => 556.0,
                    58 => 333.0,
                    59 => 333.0,
                    65 => 722.0,
                    66 => 722.0,
                    67 => 722.0,
                    68 => 722.0,
                    69 => 667.0,
                    70 => 611.0,
                    71 => 778.0,
                    72 => 722.0,
                    73 => 278.0,
                    74 => 556.0,
                    75 => 722.0,
                    76 => 611.0,
                    77 => 833.0,
                    78 => 722.0,
                    79 => 778.0,
                    80 => 667.0,
                    81 => 778.0,
                    82 => 722.0,
                    83 => 667.0,
                    84 => 611.0,
                    85 => 722.0,
                    86 => 667.0,
                    87 => 944.0,
                    88 => 667.0,
                    89 => 667.0,
                    90 => 611.0,
                    97 => 556.0,
                    98 => 611.0,
                    99 => 556.0,
                    100 => 611.0,
                    101 => 556.0,
                    102 => 333.0,
                    103 => 611.0,
                    104 => 611.0,
                    105 => 278.0,
                    106 => 278.0,
                    107 => 556.0,
                    108 => 278.0,
                    109 => 889.0,
                    110 => 611.0,
                    111 => 611.0,
                    112 => 611.0,
                    113 => 611.0,
                    114 => 389.0,
                    115 => 556.0,
                    116 => 333.0,
                    117 => 611.0,
                    118 => 556.0,
                    119 => 778.0,
                    120 => 556.0,
                    121 => 556.0,
                    122 => 500.0,
                    _ => return None,
                });
            }
            return Some(match code {
                32 => 278.0,
                33 => 278.0,
                34 => 355.0,
                44 => 278.0,
                45 => 333.0,
                46 => 278.0,
                47 => 278.0,
                48..=57 => 556.0,
                58 => 278.0,
                59 => 278.0,
                65 => 667.0,
                66 => 667.0,
                67 => 722.0,
                68 => 722.0,
                69 => 667.0,
                70 => 611.0,
                71 => 778.0,
                72 => 722.0,
                73 => 278.0,
                74 => 500.0,
                75 => 667.0,
                76 => 556.0,
                77 => 833.0,
                78 => 722.0,
                79 => 778.0,
                80 => 667.0,
                81 => 778.0,
                82 => 722.0,
                83 => 667.0,
                84 => 611.0,
                85 => 722.0,
                86 => 667.0,
                87 => 944.0,
                88 => 667.0,
                89 => 667.0,
                90 => 611.0,
                97 => 556.0,
                98 => 556.0,
                99 => 500.0,
                100 => 556.0,
                101 => 556.0,
                102 => 278.0,
                103 => 556.0,
                104 => 556.0,
                105 => 222.0,
                106 => 222.0,
                107 => 500.0,
                108 => 222.0,
                109 => 833.0,
                110 => 556.0,
                111 => 556.0,
                112 => 556.0,
                113 => 556.0,
                114 => 333.0,
                115 => 500.0,
                116 => 278.0,
                117 => 556.0,
                118 => 500.0,
                119 => 722.0,
                120 => 500.0,
                121 => 500.0,
                122 => 444.0,
                _ => return None,
            });
        }
        None
    }

    /// Get the width of the space glyph (U+0020) in font units.
    ///
    /// Returns the width in 1000ths of em per PDF spec Section 9.7.4.
    /// Used for font-aware spacing threshold calculations.
    ///
    /// Per PDF Spec Section 9.4.4, word spacing should be based on actual font metrics
    /// rather than fixed ratios. This method returns the actual space glyph width,
    /// which is used to compute adaptive TJ offset thresholds that account for
    /// different font sizes and families.
    ///
    /// # Returns
    ///
    /// The width of the space character (code 0x20) in 1000ths of em. When no
    /// real space glyph is defined — a simple font with a near-zero 0x20, or a
    /// CID font with no explicit /W entry for 0x20 — returns the 0.25 em (250)
    /// typographic default rather than the font's (often much wider) /DW.
    pub fn get_space_glyph_width(&self) -> f32 {
        // The space advance feeds the caller's geometric word-gap threshold
        // (threshold = space_width × ratio); a value that is not actually the
        // space glyph's advance skews that threshold and mis-detects word
        // boundaries.
        //
        // Type0 (CID-keyed) fonts under Identity-H/V — the encoding of nearly
        // every embedded subset — map character code 0x20 to CID 32, an
        // arbitrary font-internal glyph, NOT the space. The space glyph, if
        // present, lives at a CID reached through the font's CMap / ToUnicode,
        // never at code 0x20 (ISO 32000-2 §9.7.5.2, §9.10.2). So `cid_widths`
        // keyed by 0x20 is the advance of whatever glyph sits at CID 32 —
        // frequently ~0.5 em+ (TimesNewRomanPSMT reports 563) — and feeding it
        // into the threshold makes it so wide that real justified word gaps
        // fall below it and adjacent words glue together ("All rights reserved"
        // -> "Allrightsreserved"). For Identity-encoded Type0 fonts,
        // ignore code 0x20 entirely and use the 0.25 em typographic default. ~keep
        if self.subtype == "Type0" {
            if matches!(self.encoding, Encoding::Identity) {
                return 250.0;
            }
            // Non-Identity predefined CMap (e.g. 90ms-RKSJ-H): code 0x20 can map
            // to a real space CID, so an explicit /W entry is meaningful. ~keep
            return match self.cid_widths.as_ref().and_then(|w| w.get(&0x20)) {
                Some(&w) if w >= 50.0 => w,
                _ => 250.0,
            };
        }
        let w = self.get_glyph_width(0x20);
        // Many simple subset fonts (notably shaped Arabic from Chrome /
        // browser print) omit a glyph for code 0x20 entirely, so this returns
        // ~0. A zero width collapses the threshold to 0, so *every* inter-glyph
        // kerning gap is read as a word boundary and cursive Arabic words
        // shatter into single letters. Fall back to a typographic
        // default of 0.25 em (250 font units) — the same value
        // `should_insert_space` uses when the font is absent. ~keep
        if w < 50.0 { 250.0 } else { w }
    }

    /// Map a Glyph ID (GID) to a standard PostScript glyph name.
    ///
    /// This is used as a fallback for Type0 fonts without ToUnicode CMaps.
    /// For ASCII range GIDs (32-126), maps to standard PostScript glyph names
    /// that can be looked up in the Adobe Glyph List.
    ///
    /// Phase 1.2: Adobe Glyph List Fallback
    ///
    /// # Arguments
    ///
    /// * `gid` - The Glyph ID to map (typically 0x20-0x7E for ASCII)
    ///
    /// # Returns
    ///
    /// The standard glyph name if GID is in the ASCII range, None otherwise
    ///
    /// # Examples
    ///
    /// ```ignore
    /// assert_eq!(FontInfo::gid_to_standard_glyph_name(0x41), Some("A"));
    /// assert_eq!(FontInfo::gid_to_standard_glyph_name(0x20), Some("space"));
    /// assert_eq!(FontInfo::gid_to_standard_glyph_name(0xFFFF), None);
    /// ```
    pub fn gid_to_standard_glyph_name(gid: u16) -> Option<&'static str> {
        match gid {
            0x20 => Some("space"),
            0x21 => Some("exclam"),
            0x22 => Some("quotedbl"),
            0x23 => Some("numbersign"),
            0x24 => Some("dollar"),
            0x25 => Some("percent"),
            0x26 => Some("ampersand"),
            0x27 => Some("quoteright"),
            0x28 => Some("parenleft"),
            0x29 => Some("parenright"),
            0x2A => Some("asterisk"),
            0x2B => Some("plus"),
            0x2C => Some("comma"),
            0x2D => Some("hyphen"),
            0x2E => Some("period"),
            0x2F => Some("slash"),
            0x30 => Some("zero"),
            0x31 => Some("one"),
            0x32 => Some("two"),
            0x33 => Some("three"),
            0x34 => Some("four"),
            0x35 => Some("five"),
            0x36 => Some("six"),
            0x37 => Some("seven"),
            0x38 => Some("eight"),
            0x39 => Some("nine"),
            0x3A => Some("colon"),
            0x3B => Some("semicolon"),
            0x3C => Some("less"),
            0x3D => Some("equal"),
            0x3E => Some("greater"),
            0x3F => Some("question"),
            0x40 => Some("at"),
            0x41 => Some("A"),
            0x42 => Some("B"),
            0x43 => Some("C"),
            0x44 => Some("D"),
            0x45 => Some("E"),
            0x46 => Some("F"),
            0x47 => Some("G"),
            0x48 => Some("H"),
            0x49 => Some("I"),
            0x4A => Some("J"),
            0x4B => Some("K"),
            0x4C => Some("L"),
            0x4D => Some("M"),
            0x4E => Some("N"),
            0x4F => Some("O"),
            0x50 => Some("P"),
            0x51 => Some("Q"),
            0x52 => Some("R"),
            0x53 => Some("S"),
            0x54 => Some("T"),
            0x55 => Some("U"),
            0x56 => Some("V"),
            0x57 => Some("W"),
            0x58 => Some("X"),
            0x59 => Some("Y"),
            0x5A => Some("Z"),
            0x5B => Some("bracketleft"),
            0x5C => Some("backslash"),
            0x5D => Some("bracketright"),
            0x5E => Some("asciicircum"),
            0x5F => Some("underscore"),
            0x60 => Some("quoteleft"),
            0x61 => Some("a"),
            0x62 => Some("b"),
            0x63 => Some("c"),
            0x64 => Some("d"),
            0x65 => Some("e"),
            0x66 => Some("f"),
            0x67 => Some("g"),
            0x68 => Some("h"),
            0x69 => Some("i"),
            0x6A => Some("j"),
            0x6B => Some("k"),
            0x6C => Some("l"),
            0x6D => Some("m"),
            0x6E => Some("n"),
            0x6F => Some("o"),
            0x70 => Some("p"),
            0x71 => Some("q"),
            0x72 => Some("r"),
            0x73 => Some("s"),
            0x74 => Some("t"),
            0x75 => Some("u"),
            0x76 => Some("v"),
            0x77 => Some("w"),
            0x78 => Some("x"),
            0x79 => Some("y"),
            0x7A => Some("z"),
            0x7B => Some("braceleft"),
            0x7C => Some("bar"),
            0x7D => Some("braceright"),
            0x7E => Some("asciitilde"),

            // ==================================================================================
            // Extended Latin / Windows-1252 range (0x80-0xFF)
            // ==================================================================================
            // These mappings cover the extended ASCII characters commonly found in Western
            // European PDFs. When a Type0 font with Identity CMap encounters these GIDs,
            // we map them to their standard PostScript glyph names for AGL lookup.
            //
            // Per PDF Spec ISO 32000-1:2008 Section 9.10.2, when ToUnicode CMap is unavailable,
            // readers may use glyph name lookup as a fallback mechanism. ~keep
            0x80 => Some("euro"),
            0x82 => Some("quotesinglbase"),
            0x83 => Some("florin"),
            0x84 => Some("quotedblbase"),
            0x85 => Some("ellipsis"),
            0x86 => Some("dagger"),
            0x87 => Some("daggerdbl"),
            0x88 => Some("circumflex"),
            0x89 => Some("perthousand"),
            0x8A => Some("Scaron"),
            0x8B => Some("guilsinglleft"),
            0x8C => Some("OE"),
            0x8E => Some("Zcaron"),

            0x91 => Some("quoteleft"),
            0x92 => Some("quoteright"),
            0x93 => Some("quotedblleft"),
            0x94 => Some("quotedblright"),
            0x95 => Some("bullet"),
            0x96 => Some("endash"),
            0x97 => Some("emdash"),
            0x98 => Some("tilde"),
            0x99 => Some("trademark"),
            0x9A => Some("scaron"),
            0x9B => Some("guilsinglright"),
            0x9C => Some("oe"),
            0x9E => Some("zcaron"),
            0x9F => Some("Ydieresis"),

            0xA0 => Some("space"),
            0xA1 => Some("exclamdown"),
            0xA2 => Some("cent"),
            0xA3 => Some("sterling"),
            0xA4 => Some("currency"),
            0xA5 => Some("yen"),
            0xA6 => Some("brokenbar"),
            0xA7 => Some("section"),
            0xA8 => Some("dieresis"),
            0xA9 => Some("copyright"), // U+00A9 (Copyright sign)
            0xAA => Some("ordfeminine"),
            0xAB => Some("guillemotleft"),
            0xAC => Some("logicalnot"),
            0xAD => Some("uni00AD"),
            0xAE => Some("registered"),
            0xAF => Some("macron"),
            0xB0 => Some("degree"),
            0xB1 => Some("plusminus"),
            0xB2 => Some("twosuperior"),
            0xB3 => Some("threesuperior"),
            0xB4 => Some("acute"),
            0xB5 => Some("mu"),
            0xB6 => Some("paragraph"),
            0xB7 => Some("middot"),
            0xB8 => Some("cedilla"),
            0xB9 => Some("onesuperior"),
            0xBA => Some("ordmasculine"),
            0xBB => Some("guillemotright"),
            0xBC => Some("onequarter"),
            0xBD => Some("onehalf"),
            0xBE => Some("threequarters"),
            0xBF => Some("questiondown"),

            0xC0 => Some("Agrave"),
            0xC1 => Some("Aacute"),
            0xC2 => Some("Acircumflex"),
            0xC3 => Some("Atilde"),
            0xC4 => Some("Adieresis"),
            0xC5 => Some("Aring"),
            0xC6 => Some("AE"),
            0xC7 => Some("Ccedilla"),
            0xC8 => Some("Egrave"),
            0xC9 => Some("Eacute"),
            0xCA => Some("Ecircumflex"),
            0xCB => Some("Edieresis"),
            0xCC => Some("Igrave"),
            0xCD => Some("Iacute"),
            0xCE => Some("Icircumflex"),
            0xCF => Some("Idieresis"),
            0xD0 => Some("Eth"),
            0xD1 => Some("Ntilde"),
            0xD2 => Some("Ograve"),
            0xD3 => Some("Oacute"),
            0xD4 => Some("Ocircumflex"),
            0xD5 => Some("Otilde"),
            0xD6 => Some("Odieresis"),
            0xD7 => Some("multiply"),
            0xD8 => Some("Oslash"),
            0xD9 => Some("Ugrave"),
            0xDA => Some("Uacute"),
            0xDB => Some("Ucircumflex"),
            0xDC => Some("Udieresis"),
            0xDD => Some("Yacute"),
            0xDE => Some("Thorn"),
            0xDF => Some("germandbls"),
            0xE0 => Some("agrave"),
            0xE1 => Some("aacute"),
            0xE2 => Some("acircumflex"),
            0xE3 => Some("atilde"),
            0xE4 => Some("adieresis"),
            0xE5 => Some("aring"),
            0xE6 => Some("ae"),
            0xE7 => Some("ccedilla"),
            0xE8 => Some("egrave"),
            0xE9 => Some("eacute"),
            0xEA => Some("ecircumflex"),
            0xEB => Some("edieresis"),
            0xEC => Some("igrave"),
            0xED => Some("iacute"),
            0xEE => Some("icircumflex"),
            0xEF => Some("idieresis"),
            0xF0 => Some("eth"),
            0xF1 => Some("ntilde"),
            0xF2 => Some("ograve"),
            0xF3 => Some("oacute"),
            0xF4 => Some("ocircumflex"),
            0xF5 => Some("otilde"),
            0xF6 => Some("odieresis"),
            0xF7 => Some("divide"),
            0xF8 => Some("oslash"),
            0xF9 => Some("ugrave"),
            0xFA => Some("uacute"),
            0xFB => Some("ucircumflex"),
            0xFC => Some("udieresis"),
            0xFD => Some("yacute"),
            0xFE => Some("thorn"),
            0xFF => Some("ydieresis"),

            _ => None,
        }
    }

    /// Get the pre-computed byte→char lookup table for OneByte (simple) fonts.
    /// Built lazily on first call by running `char_to_unicode` for all 256 byte values.
    /// Returns a 256-element array: non-'\0' = single printable char, '\0' = needs fallback.
    /// Control chars (except tab/newline/cr), multi-char, and \u{FFFD} are stored as '\0'.
    pub fn get_byte_to_char_table(&self) -> &[char; 256] {
        self.byte_to_char_table.get_or_init(|| {
            let mut tbl = ['\0'; 256];
            for i in 0..=255u8 {
                if let Some(s) = self.char_to_unicode(i as u32) {
                    let mut chars = s.chars();
                    if let Some(c) = chars.next()
                        && chars.next().is_none()
                        && c != '\u{FFFD}'
                        && (c >= '\x20' || c == '\t' || c == '\n' || c == '\r')
                    {
                        tbl[i as usize] = c;
                    }
                }
            }
            tbl
        })
    }

    /// Pre-computed byte→width lookup for simple (non-Type0) fonts.
    /// Returns a 256-entry array where index i = glyph width for byte i.
    /// Eliminates per-byte bounds check and subtraction in advance_position.
    #[inline]
    pub fn get_byte_to_width_table(&self) -> &[f32; 256] {
        self.byte_to_width_table.get_or_init(|| self.declared_byte_widths())
    }

    fn declared_byte_widths(&self) -> [f32; 256] {
        let mut table = [self.default_width; 256];
        if let (Some(widths), Some(first_char)) = (&self.widths, self.first_char) {
            for (index, &width) in widths.iter().enumerate() {
                let code = first_char as usize + index;
                if code < 256 {
                    table[code] = width;
                }
            }
        } else if self.widths.is_none() {
            for byte_code in 0..256u16 {
                if let Some(width) = self.get_standard_font_width(byte_code) {
                    table[byte_code as usize] = width;
                }
            }
        }
        table
    }

    fn declared_zero_byte_codes(&self) -> Vec<u8> {
        let Some(widths) = self.widths.as_ref() else {
            return Vec::new();
        };
        let Some(first_char) = self.first_char else {
            return Vec::new();
        };
        widths
            .iter()
            .enumerate()
            .filter_map(|(index, width)| {
                let code = first_char as usize + index;
                (*width == 0.0 && code < 256).then_some(code as u8)
            })
            .collect()
    }

    fn extraction_fallback_width<'font>(
        &self,
        code: u8,
        decoded: char,
        font_ref: Option<&FontRef<'font>>,
        embedded_metrics: Option<&(Metrics, GlyphMetrics<'font>)>,
    ) -> Option<f32> {
        let embedded_width = embedded_metrics.and_then(|(metrics, glyph_metrics)| {
            let font = font_ref?;
            let gid = self
                .truetype_cmap()
                .and_then(|cmap| cmap.code_to_gid(code as u16))
                .map(GlyphId::from)
                .or_else(|| font.charmap().map(decoded))?;
            let advance = glyph_metrics.advance_width(gid)?;
            (metrics.units_per_em > 0 && advance > 0.0).then_some(advance * 1000.0 / metrics.units_per_em as f32)
        });
        embedded_width
            .or_else(|| self.get_standard_font_intrinsic_width(code as u16))
            .filter(|width| width.is_finite() && *width > 0.0)
    }

    fn repair_zero_extraction_widths(&self, widths: &mut [f32; 256]) {
        if self.subtype == "Type3" || self.subtype == "Type0" || self.wmode != 0 {
            return;
        }
        let font_ref = self
            .embedded_font_data
            .as_deref()
            .and_then(|data| FontRef::new(data).ok());
        let embedded_metrics = font_ref.as_ref().map(|font| {
            (
                Metrics::new(font, Size::unscaled(), LocationRef::default()),
                GlyphMetrics::new(font, Size::unscaled(), LocationRef::default()),
            )
        });
        for code in self.declared_zero_byte_codes() {
            let decoded = self.char_to_unicode(code as u32).and_then(|value| value.chars().next());
            if decoded.is_some_and(|character| unicode_bidi::bidi_class(character) == unicode_bidi::BidiClass::NSM) {
                continue;
            }
            let Some(width) = decoded.and_then(|character| {
                self.extraction_fallback_width(code, character, font_ref.as_ref(), embedded_metrics.as_ref())
            }) else {
                continue;
            };
            widths[code as usize] = width;
        }
    }

    /// Builds extraction-only widths without changing either public authored-width API. ~keep
    pub(crate) fn build_extraction_byte_widths(&self) -> [f32; 256] {
        let mut widths = self.declared_byte_widths();
        self.repair_zero_extraction_widths(&mut widths);
        widths
    }

    /// Convert a character code to Unicode string.
    ///
    /// Returns the faithful Unicode mapping per PDF Spec §9.10.2. Ligature
    /// characters (U+FB00–FB06) are preserved here; expansion into component
    /// letters is done by the text pipeline via `LigatureDecisionMaker`, which
    /// inspects surrounding context (neighboring chars, word boundaries) to
    /// decide whether to split — keeping font_dict a pure encoding layer.
    pub fn char_to_unicode(&self, char_code: u32) -> Option<String> {
        // Serve from the per-font memo. Read and write are separate lock scopes
        // so the decode in between never holds the lock. ~keep
        if let Ok(memo) = self.type0_unicode_memo.lock()
            && let Some(cached) = memo.get(&char_code)
        {
            return cached.clone();
        }
        let result = self
            .char_to_unicode_uncached(char_code)
            .map(|s| normalize_cjk_radical_forms(&s));
        if let Ok(mut memo) = self.type0_unicode_memo.lock() {
            memo.insert(char_code, result.clone());
        }
        result
    }

    /// Uncached decode cascade behind [`Self::char_to_unicode`].
    fn char_to_unicode_uncached(&self, char_code: u32) -> Option<String> {
        // char_code is now u32 to support 4-byte character codes (0x00000000-0xFFFFFFFF)
        // This is backward compatible - u16 values are automatically promoted to u32 ~keep

        // ==================================================================================
        // PRIORITY 1: ToUnicode CMap (PDF Spec Section 9.10.2, Method 1)
        // ==================================================================================
        //
        // Per §9.10.2: if a ToUnicode CMap is PRESENT it is the authoritative source.
        // For composite (Type0) fonts a present-but-incomplete ToUnicode means the
        // unmapped codes genuinely have no Unicode equivalent. Falling through to the
        // predefined-CMap path (Priority 3 §9.10.2) for Type0 would guess wrong CJK
        // characters and score near zero versus ground truth. Therefore:
        //
        //   • ToUnicode hit → return the mapped string (or U+FFFD if it maps to FFFD
        //     or a bare C0 control character).
        //   • ToUnicode miss AND font is Type0 → return U+FFFD, do NOT fall through.
        //   • ToUnicode miss AND font is NOT Type0 → fall through to lower priorities
        //     (simple fonts with standard encoding still benefit from further lookup).
        //
        // Fix A (§9.10.2 Priority-3 guard): implemented in the CMap-miss branch below.
        // Fix B (control-character filter): applied on CMap hits. ~keep
        if let Some(lazy_cmap) = &self.to_unicode {
            if let Some(cmap) = lazy_cmap.get() {
                let raw_unicode = cmap.get(&char_code);
                let had_hit = raw_unicode.is_some();

                // For Identity-encoded fonts, a U+FFFD (notdefrange) or a BMP
                // noncharacter (U+FFFE / U+FFFF) result is NOT a definitive
                // mapping — some producers stuff these into ToUnicode as a
                // "no glyph" placeholder (arial_unicode_ab_cidfont maps every
                // CID to U+FFFF). The CID→GID→embedded-cmap / CID-as-Unicode
                // fallback below recovers the real character, so treat them as a
                // CMap miss. Noncharacters are permanently reserved and never
                // valid text, so this can only ever improve Identity-font output. ~keep
                let effective_hit = raw_unicode.filter(|u| {
                    let is_placeholder =
                        !u.is_empty() && u.chars().all(|c| matches!(c, '\u{FFFD}' | '\u{FFFE}' | '\u{FFFF}'));
                    !(is_placeholder && matches!(self.encoding, Encoding::Identity))
                });

                if let Some(unicode) = effective_hit {
                    // Fix B: filter bare C0 control characters (U+0000–U+001F except
                    // tab/LF/CR which are legitimate whitespace in extracted text).
                    // These are never valid visible text and typically indicate a
                    // broken ToUnicode entry. Return U+FFFD and do NOT fall through
                    // even for simple fonts — the CMap explicitly mapped this code. ~keep
                    let is_c0_control = unicode
                        .chars()
                        .all(|c| matches!(c as u32, 0x00..=0x08 | 0x0B..=0x0C | 0x0E..=0x1F))
                        && !unicode.is_empty();

                    if unicode.as_ref() == "\u{FFFD}" {
                        tracing::trace!(
                            "ToUnicode CMap has U+FFFD for code 0x{:02X} in font '{}' - returning U+FFFD",
                            char_code,
                            self.base_font
                        );
                        return Some("\u{FFFD}".to_string());
                    }
                    if is_c0_control {
                        tracing::trace!(
                            "ToUnicode CMap maps code 0x{:04X} to C0 control char(s) in font '{}' - returning U+FFFD",
                            char_code,
                            self.base_font
                        );
                        return Some("\u{FFFD}".to_string());
                    }
                    // Interception A (Item 1): glyph-name-gated punctuation
                    // recovery. When a present ToUnicode CMap resolves a code to
                    // a non-sensible symbol (e.g. U+00AC `¬`) but the font's
                    // authoritative glyph name for that code is punctuation
                    // (`period`/`comma`/`hyphen`/`minus` via /Differences or the
                    // embedded post/charset table), prefer the §9.10.2(a)+(b) AGL
                    // result. Gated so a correctly-mapped period (whose hit is
                    // already `.`) never enters here. ~keep
                    if is_non_sensible_symbol(&unicode)
                        && let Some(glyph_name) = self.glyph_name_for_code(char_code)
                        && let Some(punct) = punctuation_unicode_for_glyph_name(glyph_name)
                    {
                        tracing::trace!(
                            "Interception A: code 0x{:04X} ToUnicode '{}' is a non-sensible symbol; glyph name '{}' → '{}' (font '{}')",
                            char_code,
                            unicode,
                            glyph_name,
                            punct,
                            self.base_font
                        );
                        return Some(punct.to_string());
                    }
                    return Some(unicode.into_owned());
                } else {
                    if had_hit {
                        tracing::trace!(
                            "Identity font '{}': notdefrange U+FFFD treated as miss for code 0x{:04X} — falling through to CID-as-Unicode",
                            self.base_font,
                            char_code
                        );
                    } else {
                        tracing::trace!(
                            "ToUnicode CMap MISS: font='{}' subtype='{}' code=0x{:04X} (cmap has {} entries)",
                            self.base_font,
                            self.subtype,
                            char_code,
                            cmap.len()
                        );
                    }

                    // Fix A (§9.10.2): for composite (Type0) fonts a present ToUnicode
                    // CMap is the authoritative mapping. A miss means the glyph has no
                    // Unicode equivalent — do NOT fall through to the predefined-CMap
                    // path which would produce plausible-looking but wrong CJK chars.
                    // Exception: Identity-encoded fonts map CID directly to Unicode, so
                    // a CMap miss still has a valid fallback (CID == Unicode codepoint).
                    // Blocking them here would suppress spaces and Latin characters. ~keep
                    if self.subtype == "Type0" && !matches!(self.encoding, Encoding::Identity) {
                        tracing::trace!(
                            "Type0 font '{}': ToUnicode present but code 0x{:04X} not covered → U+FFFD (no Priority-3 fallback per §9.10.2)",
                            self.base_font,
                            char_code
                        );
                        return Some("\u{FFFD}".to_string());
                    }
                }
            } else {
                // Deliberately TRACE, not WARN: `LazyCMap::get()` already
                // emits one memoized WARN naming this font when its CMap
                // fails to parse, so warning here too would report a single
                // broken ToUnicode twice. This line is per-character detail
                // for someone already debugging that font. ~keep
                tracing::trace!(
                    "Failed to parse lazy CMap for font '{}' - will fall back to Priority 2",
                    self.base_font
                );
            }
        } else if self.subtype == "Type0" {
            tracing::trace!(
                "Type0 font '{}' missing ToUnicode CMap - will fall back to Priority 2",
                self.base_font
            );
        }

        // ==================================================================================
        // PRIORITY 2: Predefined CMaps (PDF Spec Section 9.7.5.2)
        // ==================================================================================
        // Phase 3.1: Identity-H/Identity-V Predefined CMap Support
        //
        // For CID-keyed fonts (Type0 subtype), predefined CMaps provide character mapping
        // when no ToUnicode CMap is present. This is critical for CJK PDFs using standard
        // Adobe CID collections (Adobe-Identity, Adobe-GB1, Adobe-Japan1, etc.)
        //
        // Identity-H/Identity-V: The simplest predefined CMap
        // - Maps 2-byte CID directly to 2-byte Unicode code point: CID == Unicode
        // - Used with ANY font when encoding is "Identity-H" or "Identity-V"
        // - Per PDF Spec ISO 32000-1:2008 Section 9.7.5.2
        //
        // Examples:
        // - CID 0x4E00 → U+4E00 (CJK UNIFIED IDEOGRAPH "一" in Chinese/Japanese)
        // - CID 0x0041 → U+0041 (Latin Capital Letter A)
        //
        // NOTE: Identity-H/V is actually handled by checking the encoding field.
        // It is checked here for Type0 fonts to ensure it happens before other fallbacks.
        // ~keep
        if self.subtype == "Type0"
            && let Encoding::Standard(ref encoding_name) = self.encoding
            && (encoding_name == "Identity-H"
                || encoding_name == "Identity-V"
                || encoding_name.contains("UCS2")
                || encoding_name.contains("UTF16"))
        {
            // For Identity-H/V: CID value IS the Unicode code point (2-byte)
            // Valid Unicode range for 2-byte CID: 0x0000 to 0xFFFF
            // (Standard Unicode BMP - Basic Multilingual Plane)
            // Since char_code is u16, it's always in range [0x0000, 0xFFFF]
            //
            // IMPORTANT: Per PDF Spec 9.10.2, Type0 fonts require either:
            // 1. A ToUnicode CMap, OR
            // 2. A predefined CMap (which requires CIDSystemInfo)
            //
            // If neither exists, we should NOT treat Identity-H/V as valid for Type0.
            // This prevents "identity" treatment when there's no proper CIDSystemInfo. ~keep
            if self.cid_system_info.is_some() {
                // For Adobe-Identity ordering, CIDs are glyph indices (GIDs),
                // NOT Unicode code points. Try the embedded TrueType cmap first. ~keep
                let is_identity_ordering = self
                    .cid_system_info
                    .as_ref()
                    .map(|info| info.ordering == "Identity")
                    .unwrap_or(false);

                if is_identity_ordering && let Some(tt_cmap) = self.truetype_cmap() {
                    let gid = if let Some(ref cid_to_gid) = self.cid_to_gid_map {
                        cid_to_gid.get_gid(char_code as u16)
                    } else {
                        char_code as u16
                    };
                    if let Some(unicode_char) = tt_cmap.get_unicode(gid) {
                        return Some(unicode_char.to_string());
                    }
                }

                // For UCS2/UTF16 encodings, char codes ARE Unicode values directly.
                // For Identity-H/V with non-Identity ordering (e.g., Adobe-GB1),
                // char codes are CIDs that need CID-to-Unicode lookup. ~keep
                let is_ucs2_or_utf16 = encoding_name.contains("UCS2") || encoding_name.contains("UTF16");
                let is_non_identity_ordering = self
                    .cid_system_info
                    .as_ref()
                    .map(|info| info.ordering != "Identity")
                    .unwrap_or(false);

                if !is_ucs2_or_utf16 && is_non_identity_ordering {
                    // Identity-H/V with CJK collection: CIDs are NOT Unicode! ~keep
                    if let Some(unicode_codepoint) =
                        lookup_predefined_cmap(encoding_name, &self.cid_system_info, char_code as u16)
                        && let Some(unicode_char) = char::from_u32(unicode_codepoint)
                    {
                        return Some(unicode_char.to_string());
                    }
                    // CID lookup failed — fall through to Priority 2b and beyond ~keep
                } else {
                    if let Some(unicode_char) = char::from_u32(char_code)
                        && (!unicode_char.is_control() || unicode_char == ' ')
                    {
                        return Some(unicode_char.to_string());
                    }
                }
            } else {
                // No CIDSystemInfo — use CID-as-Unicode as last resort.
                // Many PDF generators assign CID values equal to Unicode code points
                // even without proper CIDSystemInfo. MuPDF uses this fallback. ~keep
                if let Some(unicode_char) = char::from_u32(char_code)
                    && (!unicode_char.is_control() || unicode_char == ' ')
                {
                    tracing::trace!(
                        "Identity-H/V CID-as-Unicode fallback (no CIDSystemInfo): font='{}' CID=0x{:04X} → '{}' (U+{:04X})",
                        self.base_font,
                        char_code,
                        unicode_char,
                        unicode_char as u32
                    );
                    return Some(unicode_char.to_string());
                }
                tracing::trace!(
                    "Type0 font '{}' with {} encoding: CID 0x{:04X} is not a valid Unicode code point",
                    self.base_font,
                    encoding_name,
                    char_code
                );
            }
        }

        // ==================================================================================
        // PRIORITY 2a: Shift-JIS (RKSJ) direct decoding
        // ==================================================================================
        // For fonts using 90ms-RKSJ-H/V encoding, the char_code is a Shift-JIS value
        // (after byte grouping in decode_text_to_unicode). Convert directly to Unicode. ~keep
        if self.subtype == "Type0"
            && let Encoding::Standard(ref enc) = self.encoding
            && enc.contains("RKSJ")
            && let Some(unicode_char) = shift_jis_to_unicode(char_code as u16)
        {
            return Some(unicode_char.to_string());
        }

        // ==================================================================================
        // PRIORITY 2b: Unicode-based Predefined CMaps (Phase 3.2)
        // ==================================================================================
        // For Type0 fonts without a ToUnicode CMap: follow PDF §9.10.2 priority order.
        //
        // The spec defines two distinct encoding CMap kinds:
        //
        //   (a) Byte-encoding CMaps (GBpc-EUC-H, GB-EUC-H, B5pc-H, EUC-H, KSC-EUC-H,
        //       etc.): the value in the content stream is a raw multi-byte code in a
        //       legacy encoding (GBK, EUC-CN, Big5, EUC-JP, EUC-KR). §9.10.2 says to
        //       map char code → CID first, but those encoding CMap tables are not
        //       embedded here. Decoding the raw bytes directly with encoding_rs is
        //       equivalent (same Unicode output) and is permitted by the spec's fallback
        //       clause: "there is no way to determine … a conforming reader may choose a
        //       character code of their choosing."
        //
        //   (b) Identity / UCS2 CMaps (Identity-H, UniGB-UCS2-H, etc.): the value in
        //       the content stream IS (or approximates) a CID. Use the Adobe-XX CID →
        //       Unicode table directly (§9.10.2 step b).
        //
        // `decode_cjk_raw_charcode` returns None for non-byte-encoding CMaps, so
        // trying it first is safe: it is a no-op for Identity/UCS2 fonts. ~keep
        if self.subtype == "Type0" {
            let enc_name = match &self.encoding {
                Encoding::Standard(name) => name.clone(),
                Encoding::Identity => "Identity-H".to_string(),
                Encoding::Custom(_) => String::new(),
            };

            // Step (a): try direct byte decode for legacy CJK byte-encoding CMaps.
            // This is the correct primary path for GBpc-EUC-H, GB-EUC-H, B5pc-H,
            // EUC-H, KSC-EUC-H, etc. Returns None for Identity/UCS2 CMaps, in
            // which case we fall through to the CID lookup below. ~keep
            if let Some(result) = decode_cjk_raw_charcode(char_code, &enc_name, &self.cid_system_info) {
                return Some(result);
            }

            // Step (b): CID → Unicode lookup for identity / UCS2 CMaps where the
            // char code in the stream is already a CID (or very close to one). ~keep
            if let Some(unicode_codepoint) = lookup_predefined_cmap(&enc_name, &self.cid_system_info, char_code as u16)
                && let Some(unicode_char) = char::from_u32(unicode_codepoint)
            {
                return Some(unicode_char.to_string());
            }
        }

        // ==================================================================================
        // PRIORITY 2: Predefined Encodings (PDF Spec Section 9.10.2, Method 2)
        // ==================================================================================
        // For symbolic fonts (Flags bit 3 set), the PDF spec requires us to IGNORE any
        // /Encoding entry and use the font's built-in encoding directly.
        //
        // PDF Spec ISO 32000-1:2008, Section 9.6.6.1:
        // "For symbolic fonts, the Encoding entry is ignored; characters are mapped directly
        // using their character codes to glyphs in the font."
        //
        // Common symbolic fonts: Symbol (Greek/math), ZapfDingbats (decorative) ~keep
        if self.is_symbolic() {
            let font_name_lower = self.base_font.to_lowercase();

            // Symbol font: Maps character codes to Greek letters and mathematical symbols
            // Standard encoding defined in PDF spec Annex D.4 ~keep
            if font_name_lower.contains("symbol") {
                if let Some(unicode_char) = symbol_encoding_lookup(char_code as u8) {
                    tracing::trace!(
                        "Symbolic font '{}': code 0x{:02X} → '{}' (U+{:04X}) [using Symbol encoding]",
                        self.base_font,
                        char_code,
                        unicode_char,
                        unicode_char as u32
                    );
                    return Some(unicode_char.to_string());
                }
            }
            // ZapfDingbats font: Maps character codes to decorative symbols
            // Standard encoding defined in PDF spec Annex D.5 ~keep
            else if (font_name_lower.contains("zapf") || font_name_lower.contains("dingbat"))
                && let Some(unicode_char) = zapf_dingbats_encoding_lookup(char_code as u8)
            {
                tracing::trace!(
                    "Symbolic font '{}': code 0x{:02X} → '{}' (U+{:04X}) [using ZapfDingbats encoding]",
                    self.base_font,
                    char_code,
                    unicode_char,
                    unicode_char as u32
                );
                return Some(unicode_char.to_string());
            }

            // For other symbolic fonts without specific encoding, fall through to /Encoding
            // (though spec says to ignore /Encoding, some PDFs may still work with it) ~keep
        }

        // ==================================================================================
        // PRIORITY 3: Font's /Encoding Entry (PDF Spec Section 9.10.2, Method 3)
        // ==================================================================================
        // For non-symbolic fonts, use the /Encoding entry which can be:
        // - A predefined encoding name (e.g., WinAnsiEncoding, MacRomanEncoding)
        // - A custom encoding dictionary with /BaseEncoding and /Differences array
        //
        // The /Differences array allows overriding specific character codes with custom
        // glyph names, which are then mapped to Unicode via the Adobe Glyph List (AGL). ~keep
        match &self.encoding {
            Encoding::Standard(name) => {
                if name == "Identity-H" || name == "Identity-V" {
                    // NOTE: Type0 fonts with Identity-H/V are handled at Priority 2 (predefined CMaps)
                    // above, so this code path is only reached for simple fonts (Type1, TrueType).
                    // Type0 fonts will have already returned at Priority 2 if the CID is valid Unicode.
                    // ~keep
                    if self.subtype == "Type0" {
                        // Priority 2 didn't map this CID. Use CID-as-Unicode fallback. ~keep
                        if let Some(unicode_char) = char::from_u32(char_code)
                            && (!unicode_char.is_control() || unicode_char == ' ')
                        {
                            tracing::trace!(
                                "Type0 font '{}' {} encoding Priority 3 CID-as-Unicode: CID 0x{:04X} → '{}' (U+{:04X})",
                                self.base_font,
                                name,
                                char_code,
                                unicode_char,
                                unicode_char as u32
                            );
                            return Some(unicode_char.to_string());
                        }
                        return Some("\u{FFFD}".to_string());
                    }
                    if let Some(ch) = char::from_u32(char_code) {
                        return Some(ch.to_string());
                    }
                }

                // For TrueType subset fonts with no /Encoding, character codes are often
                // GIDs (glyph indices), not standard encoding values. Per PDF Spec 9.6.5.4,
                // when no /Encoding exists and the font has a (3,1) cmap, character codes
                // map through the cmap. Try TrueType cmap first for these fonts. ~keep
                if (self.subtype == "TrueType" || self.subtype == "Type1")
                    && name == "StandardEncoding"
                    && let Some(tt_cmap) = self.truetype_cmap()
                {
                    let gid = tt_cmap.code_to_gid(char_code as u16).unwrap_or(char_code as u16);
                    if let Some(unicode_char) = tt_cmap.get_unicode(gid) {
                        return Some(unicode_char.to_string());
                    }
                }

                if let Some(unicode) = standard_encoding_lookup(name, char_code as u8) {
                    tracing::trace!("Standard encoding '{}': code 0x{:02X} → '{}'", name, char_code, unicode);
                    return Some(unicode);
                }
            }
            Encoding::Custom(map) => {
                if let Some(&custom_char) = map.get(&(char_code as u8)) {
                    tracing::trace!(
                        "Custom encoding: code 0x{:02X} → '{}' (U+{:04X})",
                        char_code,
                        custom_char,
                        custom_char as u32
                    );

                    // Interception B (Item 1): glyph-name-gated punctuation
                    // override. If the base/program encoding resolved this code to a
                    // non-sensible symbol but the /Differences glyph name is
                    // punctuation, the name is authoritative (ISO 32000-1 §9.6.6.1) —
                    // return the AGL punctuation so a `/period`-named code always wins
                    // as `.` regardless of how the resolved char came out. ~keep
                    if is_non_sensible_symbol(&custom_char.to_string())
                        && let Some(glyph_name) = self.diff_glyph_names.get(&(char_code as u8))
                        && let Some(punct) = punctuation_unicode_for_glyph_name(glyph_name)
                    {
                        tracing::trace!(
                            "Interception B: code 0x{:02X} resolved to non-sensible symbol '{}'; /Differences name '{}' → '{}' (font '{}')",
                            char_code,
                            custom_char,
                            glyph_name,
                            punct,
                            self.base_font
                        );
                        return Some(punct.to_string());
                    }

                    // Handle ligatures (ff, fi, fl, ffi, ffl) by expanding to component characters
                    // This is NOT in the PDF spec but improves text extraction usability ~keep
                    if is_ligature_char(custom_char)
                        && let Some(expanded) = expand_ligature_char(custom_char)
                    {
                        return Some(expanded.to_string());
                    }

                    return Some(custom_char.to_string());
                }
                if let Some(multi_str) = self.multi_char_map.get(&(char_code as u8)) {
                    return Some(multi_str.clone());
                }
            }
            Encoding::Identity => {
                // CRITICAL: Identity encoding assumes char_code == Unicode.
                // This is ONLY valid for simple fonts, NOT Type0/CID fonts.
                // Per PDF Spec ISO 32000-1:2008 Section 9.7.6.3:
                // "Type0 fonts REQUIRE ToUnicode CMaps for proper character mapping" ~keep

                if self.subtype == "Type0" {
                    // Type0 fonts: character codes are CID (glyph indices), NOT Unicode
                    // Per PDF Spec ISO 32000-1:2008 Section 9.7.4.2, when no ToUnicode CMap exists,
                    // conforming readers SHALL use the TrueType font's internal "cmap" table as fallback.
                    // This requires translating CID → GID via the CIDToGIDMap, then looking up Unicode.
                    // ~keep

                    if let Some(tt_cmap) = self.truetype_cmap() {
                        // Translate CID → GID using the CIDToGIDMap
                        // Note: CIDToGIDMap only works with u16 CIDs (2-byte codes)
                        // For CIDs > 0xFFFF, we skip CIDToGIDMap and use char_code as GID if it fits in u16
                        // ~keep
                        if char_code > 0xFFFF {
                            tracing::trace!(
                                "CID 0x{:X} in font '{}' is too large (> 0xFFFF) for CIDToGIDMap - skipping TrueType cmap",
                                char_code,
                                self.base_font
                            );
                            return None;
                        }
                        let gid = if let Some(ref cid_to_gid) = self.cid_to_gid_map {
                            cid_to_gid.get_gid(char_code as u16)
                        } else {
                            char_code as u16
                        };

                        if let Some(unicode_char) = tt_cmap.get_unicode(gid) {
                            tracing::trace!(
                                "TrueType cmap fallback SUCCESS: font='{}' CID=0x{:04X} (GID={}) → '{}' (U+{:04X})",
                                self.base_font,
                                char_code,
                                gid,
                                unicode_char,
                                unicode_char as u32
                            );
                            return Some(unicode_char.to_string());
                        } else {
                            tracing::trace!(
                                "TrueType cmap: GID {} not found in font '{}' (CID 0x{:04X} mapped via {})",
                                gid,
                                self.base_font,
                                char_code,
                                if self.cid_to_gid_map.is_some() {
                                    "explicit CIDToGIDMap"
                                } else {
                                    "Identity mapping"
                                }
                            );
                        }

                        // ==========================================================================
                        // PRIORITY 3c: embedded post/charset glyph name → AGL+synth
                        // ==========================================================================
                        // Per ISO 32000-1:2008 §9.10.2 fallback chain, consult the embedded font
                        // program's own glyph-name table when the TrueType `cmap` reverse lookup
                        // misses. Common on PowerPoint/Acrobat-exported Type0 Identity-H subset
                        // fonts that strip the Unicode `cmap` but keep `post` Format 2 names —
                        // bullets and `fi`/`fl` ligatures only recover via this path. Mirrors
                        // pdf.js / MuPDF / PDFBox 3.x behaviour. The earlier `gid_to_standard_
                        // glyph_name` (P5) only knows hardcoded ASCII-range GID → name; the post
                        // table is the font's own authoritative source. ~keep
                        if let Some(glyph_name) = self.embedded_glyph_name(gid) {
                            if let Some(unicode) = super::character_mapper::glyph_name_to_unicode(glyph_name) {
                                tracing::trace!(
                                    "Priority 3c (embedded post glyph name): font='{}' CID=0x{:04X} (GID={}) → '{}' → '{}'",
                                    self.base_font,
                                    char_code,
                                    gid,
                                    glyph_name,
                                    unicode,
                                );
                                return Some(unicode);
                            } else {
                                tracing::trace!(
                                    "Priority 3c: font='{}' GID={} → name='{}' but AGL/synth lookup failed",
                                    self.base_font,
                                    gid,
                                    glyph_name,
                                );
                            }
                        }
                    }

                    // ==================================================================================
                    // PRIORITY 5: Adobe Glyph List Fallback (Phase 1.2)
                    // ==================================================================================
                    // When TrueType cmap fails (or is not available), try Adobe Glyph List fallback.
                    // This handles Type0 fonts with standard glyph names (e.g., Aptos, LMRoman)
                    // that don't have ToUnicode CMaps or embedded TrueType fonts.
                    //
                    // Process: CID → GID (via CIDToGIDMap) → Glyph Name → Unicode (via AGL)
                    //
                    // IMPORTANT: Only apply AGL fallback if a CIDToGIDMap is explicitly defined
                    // (even if it's Identity). This distinguishes between:
                    // - Type0 fonts with proper CIDToGIDMap (may have standard glyphs)
                    // - Malformed Type0 fonts without CIDToGIDMap (unlikely to work)
                    //
                    // Per PDF Spec ISO 32000-1:2008 Section 9.10.2:
                    // "If a ToUnicode CMap is not available, conforming readers may fall back
                    // to predefined encodings and glyph name lookup." ~keep

                    // A present-but-empty /ToUnicode (0 bfchar/bfrange) maps nothing, so it
                    // counts as absent — otherwise an Identity-ordered font with an empty CMap
                    // would suppress the fallbacks below and drop all its text. ~keep
                    let has_usable_tounicode = self
                        .to_unicode
                        .as_ref()
                        .and_then(|c| c.get())
                        .is_some_and(|cmap| !cmap.is_empty());
                    let is_identity_ordered = self
                        .cid_system_info
                        .as_ref()
                        .map(|info| info.ordering == "Identity")
                        .unwrap_or(false);

                    // The GID→AGL fallback below is a numeric *guess*: it reads the GID as a
                    // codepoint via the standard glyph-name table → AGL. It is meaningless for
                    // Identity-ordered subset fonts, whose GIDs are arbitrary — a remapped GID
                    // lands on an unrelated punctuation name (e.g. "Justin" → "J)'(i#") and would
                    // shadow the CID-as-Unicode mapping below — so it is skipped there. With a
                    // usable /ToUnicode present a code reaching here is genuinely unmapped, so the
                    // guess is suppressed entirely — prefer U+FFFD so the gap is detectable.
                    // ~keep
                    if !has_usable_tounicode
                        && !is_identity_ordered
                        && let Some(ref cid_to_gid) = self.cid_to_gid_map
                    {
                        if char_code > 0xFFFF {
                            tracing::trace!(
                                "CID 0x{:X} in font '{}' is too large (> 0xFFFF) for CIDToGIDMap AGL fallback - skipping",
                                char_code,
                                self.base_font
                            );
                            // Fall through to continue fallback attempts ~keep
                        } else {
                            let gid = cid_to_gid.get_gid(char_code as u16);

                            if let Some(glyph_name) = Self::gid_to_standard_glyph_name(gid)
                                && let Some(&unicode_char) = ADOBE_GLYPH_LIST.get(glyph_name)
                            {
                                tracing::trace!(
                                    "Adobe Glyph List fallback SUCCESS: font='{}' CID=0x{:04X} (GID={}) → glyph '{}' → '{}' (U+{:04X})",
                                    self.base_font,
                                    char_code,
                                    gid,
                                    glyph_name,
                                    unicode_char,
                                    unicode_char as u32
                                );
                                return Some(unicode_char.to_string());
                            }
                        }
                    }

                    // CID-as-Unicode fallback: many producers assign CID == Unicode codepoint.
                    // Used when there is no usable /ToUnicode, and — for Identity-ordered fonts —
                    // also for uncovered whitespace (CID 0x20 → space, which producers routinely
                    // omit and is reliably U+0020; dropping it would wreck word boundaries). Any
                    // other uncovered CID in a font that *has* a /ToUnicode has no codepoint we can
                    // trust (e.g. a ligature subset slot), so it decodes to U+FFFD instead of a
                    // plausible-but-wrong, per-file-varying guess. ~keep
                    let identity_whitespace = is_identity_ordered && char_code == 0x20;
                    if (!has_usable_tounicode || identity_whitespace)
                        && let Some(unicode_char) = char::from_u32(char_code)
                        && (!unicode_char.is_control() || unicode_char == ' ')
                    {
                        tracing::trace!(
                            "Type0 font '{}' Identity encoding CID-as-Unicode fallback: CID 0x{:04X} → '{}' (U+{:04X})",
                            self.base_font,
                            char_code,
                            unicode_char,
                            unicode_char as u32
                        );
                        return Some(unicode_char.to_string());
                    }
                    // Semantically a WARN ("glyph could not be mapped"), but this
                    // is the terminal branch of the per-character decode cascade
                    // (memoized per distinct char code, not per font) — a CJK
                    // document can hit this for thousands of CIDs, so WARN would
                    // flood the consumer. ~keep
                    tracing::trace!(
                        "Type0 font '{}' using Identity encoding: CID 0x{:04X} could not be mapped to Unicode. \
                         Embedded font: {} bytes.",
                        self.base_font,
                        char_code,
                        self.embedded_font_data.as_ref().map(|d| d.len()).unwrap_or(0)
                    );
                    return Some("\u{FFFD}".to_string());
                }

                if let Some(ch) = char::from_u32(char_code) {
                    tracing::trace!(
                        "Identity encoding (simple font '{}'): code 0x{:02X} → '{}' (U+{:04X})",
                        self.base_font,
                        char_code,
                        ch,
                        ch as u32
                    );
                    return Some(ch.to_string());
                }
            }
        }

        // ==================================================================================
        // PRIORITY 4: TrueType cmap fallback for simple fonts
        // ==================================================================================
        // When all encoding-based lookups fail, try the embedded TrueType cmap as a last
        // resort. For subset fonts, character codes may be GIDs that the encoding table
        // doesn't cover. The cmap provides GID → Unicode mapping. ~keep
        if self.subtype != "Type0"
            && let Some(tt_cmap) = self.truetype_cmap()
        {
            // Symbolic TrueType fonts index glyphs by content byte through a
            // (3,0)/(1,0) symbol cmap, so the byte is not the GID. Resolve
            // byte→GID first; fall back to byte-as-GID when no symbol cmap. ~keep
            let gid = tt_cmap.code_to_gid(char_code as u16).unwrap_or(char_code as u16);
            if let Some(unicode_char) = tt_cmap.get_unicode(gid) {
                return Some(unicode_char.to_string());
            }
        }

        // ==================================================================================
        // PRIORITY 5: Fallback - No Mapping Found
        // ==================================================================================
        // If we reach here, the character is either:
        // - A control character (0x00-0x1F, 0x7F-0x9F) - intentionally omitted
        // - A character code outside all known encodings
        // - From a malformed PDF missing encoding information
        //
        // Control characters don't have visible representations, so returning None
        // (which becomes empty string) is more appropriate than returning � (U+FFFD). ~keep
        tracing::trace!(
            "No Unicode mapping for font '{}' code=0x{:02X} (symbolic={}, encoding={:?}) - likely control char",
            self.base_font,
            char_code,
            self.is_symbolic(),
            self.encoding
        );

        // ==================================================================================
        // PRIORITY 6: Unicode Ligature Fallback
        // ==================================================================================
        // If no encoding mapping was found and the raw character code falls
        // in the Unicode ligature block (U+FB00-U+FB06), decompose into the
        // component letters. This is a pure-fallback codepath — when no
        // font data identifies the glyph, standard ligature decomposition
        // is the safest recovery. LaTeX and scientific PDF producers emit
        // these codes directly. ~keep
        let ligature_components = match char_code {
            0xFB00 => Some("ff"),
            0xFB01 => Some("fi"),
            0xFB02 => Some("fl"),
            0xFB03 => Some("ffi"),
            0xFB04 => Some("ffl"),
            0xFB05 | 0xFB06 => Some("st"),
            _ => None,
        };
        if let Some(s) = ligature_components {
            return Some(s.to_string());
        }

        None
    }

    /// Determine the font weight using a comprehensive cascade of PDF spec methods.
    ///
    /// Priority order per PDF Spec ISO 32000-1:2008:
    /// 1. FontWeight field from FontDescriptor (Table 122) - MOST RELIABLE
    /// 2. ForceBold flag (bit 19) from Flags field (Table 123)
    /// 3. Font name heuristics (fallback for legacy PDFs)
    /// 4. StemV analysis (stem thickness correlates with weight)
    ///
    /// # Returns
    ///
    /// FontWeight enum value (Thin to Black scale)
    ///
    /// # PDF Spec References
    ///
    /// - Table 122 (page 456): FontWeight values 100-900
    /// - Table 123 (page 457): ForceBold flag at bit 19 (0x80000)
    /// - Section 9.6.2: StemV field interpretation
    pub fn get_font_weight(&self) -> FontWeight {
        *self.weight_memo.get_or_init(|| self.compute_font_weight())
    }

    /// Uncached [`Self::get_font_weight`] body. Everything it reads is fixed at
    /// font-load time, so `weight_memo` can hold the answer for the font's life.
    fn compute_font_weight(&self) -> FontWeight {
        if let Some(weight_value) = self.font_weight {
            return FontWeight::from_pdf_value(weight_value);
        }

        if let Some(flags_value) = self.flags {
            const FORCE_BOLD_BIT: i32 = 0x80000;
            if (flags_value & FORCE_BOLD_BIT) != 0 {
                tracing::debug!("Font '{}': ForceBold flag set (bit 19) → Bold", self.base_font);
                return FontWeight::Bold;
            }
        }

        let name_lower = self.base_font.to_lowercase();

        if name_lower.contains("black") || name_lower.contains("heavy") {
            return FontWeight::Black;
        }
        if name_lower.contains("extrabold") || name_lower.contains("ultrabold") {
            return FontWeight::ExtraBold;
        }
        if name_lower.contains("bold") {
            if name_lower.contains("semibold") || name_lower.contains("demibold") {
                return FontWeight::SemiBold;
            }
            return FontWeight::Bold;
        }
        if name_lower.contains("medium") {
            return FontWeight::Medium;
        }
        if name_lower.contains("light") {
            if name_lower.contains("extralight") || name_lower.contains("ultralight") {
                return FontWeight::ExtraLight;
            }
            return FontWeight::Light;
        }
        if name_lower.contains("thin") {
            return FontWeight::Thin;
        }

        // ==================================================================================
        // PRIORITY 4: StemV Analysis (EXPERIMENTAL)
        // ==================================================================================
        // StemV measures vertical stem thickness. Empirically:
        // - StemV > 110: Usually bold (700+)
        // - StemV 80-110: Medium (500-600)
        // - StemV < 80: Normal or lighter (400-)
        //
        // NOTE: This is a heuristic and may not be reliable for all fonts.
        // PDF spec does not mandate this correlation. ~keep
        if let Some(stem_v) = self.stem_v {
            tracing::debug!("Font '{}': Using StemV analysis (StemV={})", self.base_font, stem_v);

            if stem_v > 110.0 {
                return FontWeight::Bold;
            } else if stem_v >= 80.0 {
                return FontWeight::Medium;
            }
            // If StemV < 80, continue to default (Normal) ~keep
        }

        FontWeight::Normal
    }

    /// Check if this font is bold (convenience method).
    ///
    /// Returns true if font weight is SemiBold (600) or higher.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// if font.is_bold() {
    ///     // Apply bold markdown formatting
    /// }
    /// ```
    pub fn is_bold(&self) -> bool {
        self.get_font_weight().is_bold()
    }

    /// Return true when this font's per-glyph widths come from the PDF's
    /// `/Widths` array (for simple fonts) or `/W` array (for Type0/CID
    /// fonts), rather than from the generic 500/550/600-thousandths-of-em
    /// fallback that `FontInfo::new` uses when neither is present.
    ///
    /// Callers use this to decide whether `byte_to_width_table` is
    /// trustworthy: when it returns false, every glyph reports the same
    /// fallback advance, so bounding-box widths computed from those
    /// advances systematically over- or under-estimate the visible text
    /// extent. On affected PDFs that collapses the real gap between
    /// adjacent `Tj`-positioned words, gluing words together in
    /// `extract_text` output even though the PDF itself places them on
    /// distinct positions.
    pub fn has_explicit_widths(&self) -> bool {
        // F14 fix: return true only when the font actually has explicit width data.
        // Previously returned true for ALL Type0 fonts, which disabled gap-correction
        // for Type0 fonts with no /W or /DW — exactly the fonts that need correction.
        // Now: true when /Widths is present (simple fonts), or when /W has entries
        // (CID fonts), or when /DW was explicitly set in the CIDFont dictionary. ~keep
        self.widths.is_some() || self.cid_widths.is_some() || self.has_explicit_dw
    }

    /// Check if this font is likely italic based on the font name.
    ///
    /// This is a heuristic check looking for "Italic" or "Oblique" in the font name.
    pub fn is_italic(&self) -> bool {
        *self.italic_memo.get_or_init(|| {
            let name_lower = self.base_font.to_lowercase();
            name_lower.contains("italic") || name_lower.contains("oblique")
        })
    }

    /// Check if this is a symbolic font based on FontDescriptor flags.
    ///
    /// Symbolic fonts (bit 3 set in /Flags) contain glyphs outside the Adobe standard
    /// Latin character set. For symbolic fonts, the PDF spec requires ignoring any
    /// Encoding entry and using direct character code mapping to the font's built-in encoding.
    ///
    /// Common symbolic fonts: Symbol, ZapfDingbats
    ///
    /// PDF Spec: ISO 32000-1:2008, Table 5.20 - Font descriptor flags
    /// Bit 3: Symbolic - Font contains glyphs outside Adobe standard Latin character set
    /// Bit 6: Nonsymbolic - Font uses Adobe standard Latin character set (mutually exclusive with bit 3)
    pub fn is_symbolic(&self) -> bool {
        if let Some(flags_value) = self.flags {
            // Bit 3 = 0x04 (1 << 2, since bits are numbered starting at 1 in PDF spec) ~keep
            const SYMBOLIC_BIT: i32 = 1 << 2;
            return (flags_value & SYMBOLIC_BIT) != 0;
        }

        let name_lower = self.base_font.to_lowercase();
        name_lower.contains("symbol") || name_lower.contains("zapf") || name_lower.contains("dingbat")
    }

    /// Get character from encoding (custom or standard).
    ///
    /// Custom encoding support
    ///
    /// This method normalizes a raw character code through the font's encoding,
    /// converting it to the actual Unicode character. This ensures word boundary
    /// detection works on real characters, not raw byte codes.
    ///
    /// Per PDF Spec ISO 32000-1:2008, Section 9.6.6:
    /// - Custom encodings with /Differences override standard encodings
    /// - Standard encodings have well-defined mappings
    /// - Identity encoding passes codes through as-is
    ///
    /// # Arguments
    ///
    /// * `code` - The raw byte value from the PDF content stream
    ///
    /// # Returns
    ///
    /// The normalized Unicode character, or None if no mapping exists
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use xberg_native_pdf::fonts::FontInfo;
    ///
    /// let font_info = /* ... load font ... */;
    /// if let Some(ch) = font_info.get_encoded_char(0x64) {
    ///     println!("Code 0x64 maps to: {}", ch);
    /// }
    /// ```
    pub fn get_encoded_char(&self, code: u8) -> Option<char> {
        match &self.encoding {
            Encoding::Custom(mappings) => mappings.get(&code).copied(),
            Encoding::Standard(_encoding_name) => {
                // Standard encoding: for now, assume ToUnicode CMap handles this
                // If we need explicit standard encoding tables, add them here
                // For basic ASCII range, we can pass through ~keep
                if code < 128 { Some(code as char) } else { None }
            }
            Encoding::Identity => {
                if code < 128 {
                    Some(code as char)
                } else {
                    None
                }
            }
        }
    }

    /// Check if font has custom encoding.
    ///
    /// Custom encoding support
    ///
    /// Returns true if the font uses a custom encoding with /Differences array,
    /// which overrides standard encoding for specific character codes.
    ///
    /// # Returns
    ///
    /// true if the font has a custom encoding, false otherwise
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use xberg_native_pdf::fonts::FontInfo;
    ///
    /// let font_info = /* ... load font ... */;
    /// if font_info.has_custom_encoding() {
    ///     println!("Font uses custom encoding");
    /// }
    /// ```
    pub fn has_custom_encoding(&self) -> bool {
        matches!(self.encoding, Encoding::Custom(_))
    }
}

/// Map a PDF glyph name to a Unicode character.
///
/// This function implements the Adobe Glyph List (AGL) specification,
/// which defines standard mappings from PostScript glyph names to Unicode.
/// This is essential for parsing /Differences arrays in custom encodings.
///
/// # Arguments
///
/// * `glyph_name` - The PostScript glyph name (e.g., "bullet", "emdash", "Aacute")
///
/// # Returns
///
/// The corresponding Unicode character, or None if the glyph name is not recognized.
///
/// # References
///
/// - Adobe Glyph List Specification: https://github.com/adobe-type-tools/agl-specification
/// - PDF 32000-1:2008, Section 9.6.6.2 (Differences Arrays)
///
/// # Examples
///
/// ```ignore
/// # use xberg_native_pdf::fonts::font_dict::glyph_name_to_unicode;
/// assert_eq!(glyph_name_to_unicode("bullet"), Some('•'));
/// assert_eq!(glyph_name_to_unicode("emdash"), Some('—'));
/// assert_eq!(glyph_name_to_unicode("A"), Some('A'));
/// assert_eq!(glyph_name_to_unicode("unknown"), None);
/// ```ignore
///
/// Extended glyph names from TeX/math fonts (MSAM, MSBM, Computer Modern, etc.)
/// not present in the standard Adobe Glyph List.
static TEX_MATH_GLYPH_NAMES: phf::Map<&'static str, char> = phf::phf_map! {
    "square" => '\u{25A1}',
    "squaredot" => '\u{22A1}',
    "blacksquare" => '\u{25A0}',
    "dblarrowup" => '\u{21C8}',
    "dblarrowdwn" => '\u{21CA}',
    "dblarrowleft" => '\u{21C7}',
    "dblarrowright" => '\u{21C9}',
    "triangle" => '\u{25B3}',
    "triangledown" => '\u{25BD}',
    "triangleleft" => '\u{25C1}',
    "triangleright" => '\u{25B7}',
    "blacktriangle" => '\u{25B2}',
    "blacktriangledown" => '\u{25BC}',
    "blacktriangleleft" => '\u{25C0}',
    "blacktriangleright" => '\u{25B6}',
    "diamond" => '\u{25C7}',
    "blackdiamond" => '\u{25C6}',
    "circle" => '\u{25CB}',
    "bullet1" => '\u{2219}',
    "star" => '\u{22C6}',
    "bigstar" => '\u{2605}',
    "checkmark" => '\u{2713}',
    "maltese" => '\u{2720}',
    "arrowleft" => '\u{2190}',
    "arrowright" => '\u{2192}',
    "arrowup" => '\u{2191}',
    "arrowdown" => '\u{2193}',
    "arrowboth" => '\u{2194}',
    "arrowdblup" => '\u{21D1}',
    "arrowdbldown" => '\u{21D3}',
    "arrowdblleft" => '\u{21D0}',
    "arrowdblright" => '\u{21D2}',
    "arrowdblboth" => '\u{21D4}',
    "langle" => '\u{27E8}',
    "rangle" => '\u{27E9}',
    "lfloor" => '\u{230A}',
    "rfloor" => '\u{230B}',
    "lceil" => '\u{2308}',
    "rceil" => '\u{2309}',
    "emptyset" => '\u{2205}',
    "infty" => '\u{221E}',
    "nabla" => '\u{2207}',
    "partial" => '\u{2202}',
    "forall" => '\u{2200}',
    "exists" => '\u{2203}',
    "neg" => '\u{00AC}',
    "backslash" => '\u{005C}',
    "prime" => '\u{2032}',
    "natural" => '\u{266E}',
    "flat" => '\u{266D}',
    "sharp" => '\u{266F}',
};

/// Convert a Shift-JIS encoded byte sequence (1 or 2 bytes) to a Unicode character.
/// Uses the encoding_rs crate for correct, complete Shift-JIS decoding.
fn shift_jis_to_unicode(code: u16) -> Option<char> {
    let bytes = if code <= 0xFF {
        vec![code as u8]
    } else {
        vec![(code >> 8) as u8, (code & 0xFF) as u8]
    };
    let (decoded, _, had_errors) = encoding_rs::SHIFT_JIS.decode(&bytes);
    if had_errors {
        return None;
    }
    let mut chars = decoded.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    Some(c)
}

/// Normalize CJK radical "presentation" codepoints to their canonical unified
/// ideograph: CJK Radicals Supplement (U+2E80–2EFF) and Kangxi Radicals
/// (U+2F00–2FDF). These blocks hold the radical glyphs used in dictionaries and
/// are never part of running text — but a font cmap that maps a glyph shared
/// between a radical and its ideograph to the *radical* codepoint (and a
/// GID→Unicode reverse lookup that then prefers it) surfaces e.g. 欠→⽋, 立→⽴.
///
/// The mapping is Unicode's `Equivalent_Unified_Ideograph` property, not NFKC:
/// NFKC decomposes the whole Kangxi block but only 2 of the 115 Supplement
/// codepoints, so an NFKC-based pass silently no-ops on e.g. ⻘ (U+2ED8).
/// Fast-path returns the input untouched when it contains no radical-block char.
fn normalize_cjk_radical_forms(s: &str) -> String {
    use super::radical_forms::radical_to_unified_ideograph;
    if !s.chars().any(|c| matches!(c as u32, 0x2E80..=0x2FDF)) {
        return s.to_string();
    }
    s.chars()
        .map(|c| radical_to_unified_ideograph(c).unwrap_or(c))
        .collect()
}

pub(crate) fn glyph_name_to_unicode(glyph_name: &str) -> Option<char> {
    // Priority 1: Adobe Glyph List (AGL) lookup - O(1) with perfect hash
    // PDF Spec: ISO 32000-1:2008, Section 9.10.2 ~keep
    if let Some(&unicode_char) = super::adobe_glyph_list::ADOBE_GLYPH_LIST.get(glyph_name) {
        return Some(unicode_char);
    }

    // Priority 1b: Extended glyph names from TeX/math fonts (MSAM, MSBM, etc.)
    // These are well-known glyph names not in the standard AGL but common in
    // academic/mathematical PDFs generated by TeX/LaTeX. ~keep
    if let Some(&unicode_char) = TEX_MATH_GLYPH_NAMES.get(glyph_name) {
        return Some(unicode_char);
    }

    // Priority 2: Parse "uniXXXX" format (e.g., uni0041 -> A)
    // Common in custom fonts and font subsets ~keep
    if glyph_name.starts_with("uni")
        && glyph_name.len() == 7
        && let Ok(code_point) = u32::from_str_radix(&glyph_name[3..], 16)
        && let Some(c) = char::from_u32(code_point)
    {
        return Some(c);
    }

    // Priority 3: Parse "uXXXX" format (e.g., u0041 -> A)
    // Alternative format used by some PDF generators ~keep
    if glyph_name.starts_with('u')
        && glyph_name.len() >= 5
        && let Ok(code_point) = u32::from_str_radix(&glyph_name[1..], 16)
        && let Some(c) = char::from_u32(code_point)
    {
        return Some(c);
    }

    // Priority 4: Underscore-delimited compound glyph names (AGL spec section 2)
    // e.g. "f_f" → 'f'+'f', "f_i" → 'f'+'i', "T_h" → 'T'+'h'
    // Return the first component character for single-char return type ~keep
    if glyph_name.contains('_') {
        let parts: Vec<&str> = glyph_name.split('_').collect();
        if let Some(first) = parts.first()
            && let Some(&ch) = super::adobe_glyph_list::ADOBE_GLYPH_LIST.get(*first)
        {
            return Some(ch);
        }
    }

    // Priority 5: delegate to the unified fallback chain
    // in `character_mapper::glyph_name_to_unicode`. The newer chain adds:
    //   - Variant-suffix stripping (`A.sc`, `bullet.alt`, `fi.001`) — common in
    //     subset fonts where producers append stylistic-variant tags.
    //   - Stricter `uniXXXX` (exactly 4 hex, no control chars) and `uXXXXX`
    //     (4..6 hex, no surrogates, no control chars) validation.
    // This brings simple-font / Type1 / CFF / Differences-array callers (which
    // route through this `font_dict::glyph_name_to_unicode` entry) onto the
    // same fallback chain as the Type0 Identity-H path. Inline-
    // image font streams (PDF spec §8.9.7) that resolve glyph names by this
    // path inherit the same behaviour transparently — no separate inline-image
    // codepath exists in this crate; inline images per spec carry only image
    // data, but any future inline-image font-resolution callsite will use this
    // unified chain by construction. ~keep
    if let Some(unicode_str) = super::character_mapper::glyph_name_to_unicode(glyph_name) {
        // The newer chain returns `String` (to allow multi-codepoint AGL
        // entries like ligatures, though current AGL values are all single
        // BMP codepoints). For the legacy `Option<char>` surface we only
        // forward if the result is exactly one `char` — multi-codepoint
        // results are handled by `glyph_name_to_unicode_string` below. ~keep
        let mut chars = unicode_str.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            return Some(c);
        }
    }

    tracing::trace!("Unknown glyph name not in Adobe Glyph List: '{}'", glyph_name);
    None
}

/// Resolve a glyph name to a Unicode string, handling compound names.
///
/// Like `glyph_name_to_unicode` but returns a full String for compound glyph names
/// (underscore-delimited per AGL spec, e.g. "f_f" → "ff", "f_f_i" → "ffi").
pub(crate) fn glyph_name_to_unicode_string(glyph_name: &str) -> Option<String> {
    if let Some(ch) = glyph_name_to_unicode(glyph_name) {
        return Some(ch.to_string());
    }

    // Handle underscore-delimited compound names (AGL spec section 2) ~keep
    if glyph_name.contains('_') {
        let mut result = String::new();
        for part in glyph_name.split('_') {
            // If any component is unknown, fail entirely. ~keep
            let ch = glyph_name_to_unicode(part)?;
            result.push(ch);
        }
        if !result.is_empty() {
            return Some(result);
        }
    }

    // Final fallback: unified chain — variant-suffix
    // stripping + strict uniXXXX / uXXXXX synth. Returns the full `String` shape
    // (multi-codepoint AGL entries are forwarded unchanged). ~keep
    super::character_mapper::glyph_name_to_unicode(glyph_name)
}

/// AGL Unicode for the closed set of punctuation glyph names that the Item 1
/// fix recovers (ISO 32000-1 §9.10.2(a)+(b)). Restricted deliberately to these
/// four names — generalising to all AGL names would re-introduce regression risk
/// against fonts whose ToUnicode is genuinely authoritative.
///
/// `period`→`"."`, `comma`→`","`, `hyphen`→`"-"`, `minus`→`"\u{2212}"`;
/// anything else → `None`.
fn punctuation_unicode_for_glyph_name(name: &str) -> Option<&'static str> {
    match name {
        "period" => Some("."),
        "comma" => Some(","),
        "hyphen" => Some("-"),
        "minus" => Some("\u{2212}"),
        _ => None,
    }
}

/// True iff `s` is a single character that is a "non-sensible symbol" — i.e. a
/// symbol/arrow/math glyph that is clearly not the punctuation a `period`/
/// `comma`/`hyphen`/`minus` glyph name denotes. This gates the Item 1
/// interceptions so they fire only when an upstream decode produced a wrong
/// symbol (e.g. U+00AC `¬` or an arrow/math char) for a punctuation-named code.
///
/// Covers the Latin-1 supplement symbol range (U+00A1..=U+00BF, which includes
/// U+00AC `¬`) and the arrow/math/symbol blocks (U+2190..=U+2BFF). Returns
/// `false` for `.`, `,`, `-`, ASCII digits, and any alphabetic letter.
fn is_non_sensible_symbol(s: &str) -> bool {
    let mut chars = s.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        // Empty or multi-char strings are not single non-sensible symbols. ~keep
        return false;
    };
    if c.is_alphabetic() || c.is_ascii_digit() || c.is_ascii_punctuation() {
        return false;
    }
    let cp = c as u32;
    matches!(cp, 0x00A1..=0x00BF | 0x2190..=0x2BFF)
}

// The old match-based glyph_name_to_unicode implementation has been replaced
// with a lookup in the complete Adobe Glyph List static map.
// See super::adobe_glyph_list::ADOBE_GLYPH_LIST for the new implementation. ~keep

/// Check if a character is a ligature.
///
/// This function identifies Unicode ligature characters (U+FB00 to U+FB06)
/// that are commonly used in PDFs for typographic ligatures.
///
/// # Arguments
///
/// * `c` - The character to check
///
/// # Returns
///
/// `true` if the character is a ligature, `false` otherwise.
///
/// # Examples
///
/// ```ignore
/// # use xberg_native_pdf::fonts::font_dict::is_ligature_char;
/// assert_eq!(is_ligature_char('ﬁ'), true); // U+FB01
/// assert_eq!(is_ligature_char('ﬂ'), true); // U+FB02
/// assert_eq!(is_ligature_char('A'), false);
/// ```ignore
fn is_ligature_char(c: char) -> bool {
    matches!(c, 'ﬀ' | 'ﬁ' | 'ﬂ' | 'ﬃ' | 'ﬄ' | 'ﬅ' | 'ﬆ')
}

/// Expand a ligature character to its ASCII equivalent.
///
/// This function handles the Unicode ligature characters (U+FB00 to U+FB06)
/// and expands them to their multi-character ASCII equivalents.
///
/// # Arguments
///
/// * `c` - The character to potentially expand
///
/// # Returns
///
/// The expanded string if `c` is a ligature, None otherwise.
///
/// # Examples
///
/// ```ignore
/// # use xberg_native_pdf::fonts::font_dict::expand_ligature_char;
/// assert_eq!(expand_ligature_char('ﬁ'), Some("fi"));
/// assert_eq!(expand_ligature_char('ﬂ'), Some("fl"));
/// assert_eq!(expand_ligature_char('A'), None);
/// ```ignore
fn expand_ligature_char(c: char) -> Option<&'static str> {
    match c {
        'ﬀ' => Some("ff"),
        'ﬁ' => Some("fi"),
        'ﬂ' => Some("fl"),
        'ﬃ' => Some("ffi"),
        'ﬄ' => Some("ffl"),
        'ﬅ' => Some("st"),
        'ﬆ' => Some("st"),
        _ => None,
    }
}

/// Expand a Unicode ligature character code to its ASCII equivalent.
///
/// This function handles the Unicode ligature character codes (U+FB00 to U+FB04)
/// and expands them to their multi-character ASCII equivalents.
///
/// This is the u16 character code variant, used in the character mapping priority chain
/// where character codes come as u16 values directly from the PDF.
///
/// # Arguments
///
/// * `char_code` - The character code (as u16) to potentially expand
///
/// # Returns
///
/// The expanded string if `char_code` is a ligature, None otherwise.
///
/// # Examples
/// Look up a character in the Adobe Symbol font encoding.
///
/// This function implements the Symbol font encoding table as defined in
/// PDF Specification Appendix D.4 (ISO 32000-1:2008, pages 996-997).
///
/// Symbol font is used extensively in mathematical and scientific documents
/// for Greek letters, mathematical operators, and special symbols.
///
/// # Arguments
///
/// * `code` - The character code (0-255)
///
/// # Returns
///
/// The corresponding Unicode character, or None if not in the encoding.
///
/// # References
///
/// - PDF 32000-1:2008, Appendix D.4 - Symbol Encoding
///
/// # Examples
///
/// ```ignore
/// # use xberg_native_pdf::fonts::font_dict::symbol_encoding_lookup;
/// assert_eq!(symbol_encoding_lookup(0x72), Some('ρ')); // rho
/// assert_eq!(symbol_encoding_lookup(0x61), Some('α')); // alpha
/// assert_eq!(symbol_encoding_lookup(0xF2), Some('∫')); // integral
/// ```ignore
fn symbol_encoding_lookup(code: u8) -> Option<char> {
    match code {
        0x61 => Some('α'),
        0x62 => Some('β'),
        0x63 => Some('χ'),
        0x64 => Some('δ'),
        0x65 => Some('ε'),
        0x66 => Some('φ'),
        0x67 => Some('γ'),
        0x68 => Some('η'),
        0x69 => Some('ι'),
        0x6A => Some('ϕ'),
        0x6B => Some('κ'),
        0x6C => Some('λ'),
        0x6D => Some('μ'),
        0x6E => Some('ν'),
        0x6F => Some('ο'),
        0x70 => Some('π'),
        0x71 => Some('θ'),
        0x72 => Some('ρ'),
        0x73 => Some('σ'),
        0x74 => Some('τ'),
        0x75 => Some('υ'),
        0x76 => Some('ϖ'),
        0x77 => Some('ω'),
        0x78 => Some('ξ'),
        0x79 => Some('ψ'),
        0x7A => Some('ζ'),

        0x41 => Some('Α'),
        0x42 => Some('Β'),
        0x43 => Some('Χ'),
        0x44 => Some('Δ'),
        0x45 => Some('Ε'),
        0x46 => Some('Φ'),
        0x47 => Some('Γ'),
        0x48 => Some('Η'),
        0x49 => Some('Ι'),
        0x4B => Some('Κ'),
        0x4C => Some('Λ'),
        0x4D => Some('Μ'),
        0x4E => Some('Ν'),
        0x4F => Some('Ο'),
        0x50 => Some('Π'),
        0x51 => Some('Θ'),
        0x52 => Some('Ρ'),
        0x53 => Some('Σ'),
        0x54 => Some('Τ'),
        0x55 => Some('Υ'),
        0x57 => Some('Ω'),
        0x58 => Some('Ξ'),
        0x59 => Some('Ψ'),
        0x5A => Some('Ζ'),

        0xB1 => Some('±'),
        0xB4 => Some('÷'),
        0xB5 => Some('∞'),
        0xB6 => Some('∂'),
        0xB7 => Some('•'),
        0xB9 => Some('≠'),
        0xBA => Some('≡'),
        0xBB => Some('≈'),
        0xBC => Some('…'),
        0xBE => Some('⊥'),
        0xBF => Some('⊙'),

        0xD0 => Some('°'),
        0xD1 => Some('∇'),
        0xD2 => Some('¬'),
        0xD3 => Some('∧'),
        0xD4 => Some('∨'),
        0xD5 => Some('∏'),
        0xD6 => Some('√'),
        0xD7 => Some('⋅'),
        0xD8 => Some('⊕'),
        0xD9 => Some('⊗'),

        0xDA => Some('∈'),
        0xDB => Some('∉'),
        0xDC => Some('∠'),
        0xDD => Some('∇'),
        0xDE => Some('®'),
        0xDF => Some('©'), // copyright
        0xE0 => Some('™'),

        0xE1 => Some('∑'),
        0xE2 => Some('⊂'),
        0xE3 => Some('⊃'),
        0xE4 => Some('⊆'),
        0xE5 => Some('⊇'),
        0xE6 => Some('∪'),
        0xE7 => Some('∩'),
        0xE8 => Some('∀'),
        0xE9 => Some('∃'),
        0xEA => Some('¬'),

        0xF1 => Some('〈'),
        0xF2 => Some('∫'),
        0xF3 => Some('⌠'),
        0xF4 => Some('⌡'),
        0xF5 => Some('⊓'),
        0xF6 => Some('⊔'),
        0xF7 => Some('〉'),

        0x20 => Some(' '),
        0x21 => Some('!'),
        0x22 => Some('∀'),
        0x23 => Some('#'),
        0x24 => Some('∃'),
        0x25 => Some('%'),
        0x26 => Some('&'),
        0x27 => Some('∋'),
        0x28 => Some('('),
        0x29 => Some(')'),
        0x2A => Some('∗'),
        0x2B => Some('+'),
        0x2C => Some(','),
        0x2D => Some('−'),
        0x2E => Some('.'),
        0x2F => Some('/'),

        0x30..=0x39 => Some(code as char),

        0x3A => Some(':'),
        0x3B => Some(';'),
        0x3C => Some('<'),
        0x3D => Some('='),
        0x3E => Some('>'),
        0x3F => Some('?'),

        0x40 => Some('≅'),

        0x5B => Some('['),
        0x5C => Some('∴'),
        0x5D => Some(']'),
        0x5E => Some('⊥'),
        0x5F => Some('_'),

        0x7B => Some('{'),
        0x7C => Some('|'),
        0x7D => Some('}'),
        0x7E => Some('∼'),

        // Math operators previously missing from the Adobe Symbol set (Annex D.5). ~keep
        0xA3 => Some('\u{2264}'),
        0xA5 => Some('\u{221E}'),
        0xB3 => Some('\u{2265}'),

        _ => None,
    }
}

/// Look up a character in the Adobe ZapfDingbats font encoding.
///
/// This function implements a subset of the ZapfDingbats font encoding table
/// as defined in PDF Specification Appendix D.5 (ISO 32000-1:2008, page 998).
///
/// ZapfDingbats font is used for ornamental symbols, arrows, and decorative characters.
///
/// # Arguments
///
/// * `code` - The character code (0-255)
///
/// # Returns
///
/// The corresponding Unicode character, or None if not in the encoding.
///
/// # References
///
/// - PDF 32000-1:2008, Appendix D.5 - ZapfDingbats Encoding
fn zapf_dingbats_encoding_lookup(code: u8) -> Option<char> {
    match code {
        0x20 => Some(' '),
        0x21 => Some('✁'),
        0x22 => Some('✂'),
        0x23 => Some('✃'),
        0x24 => Some('✄'),
        0x25 => Some('☎'),
        0x26 => Some('✆'),
        0x27 => Some('✇'),
        0x28 => Some('✈'),
        0x29 => Some('✉'),
        0x2A => Some('☛'),
        0x2B => Some('☞'),
        0x2C => Some('✌'),
        0x2D => Some('✍'),
        0x2E => Some('✎'),
        0x2F => Some('✏'),

        0x30 => Some('✐'),
        0x31 => Some('✑'),
        0x32 => Some('✒'),
        0x33 => Some('✓'),
        0x34 => Some('✔'),
        0x35 => Some('✕'),
        0x36 => Some('✖'),
        0x37 => Some('✗'),
        0x38 => Some('✘'),
        0x39 => Some('✙'),
        0x3A => Some('✚'),
        0x3B => Some('✛'),
        0x3C => Some('✜'),
        0x3D => Some('✝'),
        0x3E => Some('✞'),
        0x3F => Some('✟'),

        0x40 => Some('✠'),
        0x41 => Some('✡'),
        0x42 => Some('✢'),
        0x43 => Some('✣'),
        0x44 => Some('✤'),
        0x45 => Some('✥'),
        0x46 => Some('✦'),
        0x47 => Some('✧'),
        0x48 => Some('★'),
        0x49 => Some('✩'),
        0x4A => Some('✪'),
        0x4B => Some('✫'),
        0x4C => Some('✬'),
        0x4D => Some('✭'),
        0x4E => Some('✮'),
        0x4F => Some('✯'),

        0x50 => Some('✰'),
        0x51 => Some('✱'),
        0x52 => Some('✲'),
        0x53 => Some('✳'),
        0x54 => Some('✴'),
        0x55 => Some('✵'),
        0x56 => Some('✶'),
        0x57 => Some('✷'),
        0x58 => Some('✸'),
        0x59 => Some('✹'),
        0x5A => Some('✺'),
        0x5B => Some('✻'),
        0x5C => Some('✼'),
        0x5D => Some('✽'),
        0x5E => Some('✾'),
        0x5F => Some('✿'),

        0x60 => Some('❀'),
        0x61 => Some('❁'),
        0x62 => Some('❂'),
        0x63 => Some('❃'),
        0x64 => Some('❄'),
        0x65 => Some('❅'),
        0x66 => Some('❆'),
        0x67 => Some('❇'),
        0x68 => Some('❈'),
        0x69 => Some('❉'),
        0x6A => Some('❊'),
        0x6B => Some('❋'),

        0x6C => Some('●'),
        0x6D => Some('○'),
        0x6E => Some('❍'),
        0x6F => Some('■'),
        0x70 => Some('□'),
        0x71 => Some('▢'),
        0x72 => Some('▣'),
        0x73 => Some('▤'),
        0x74 => Some('▥'),
        0x75 => Some('▦'),
        0x76 => Some('▧'),
        0x77 => Some('▨'),
        0x78 => Some('▩'),
        0x79 => Some('▪'),
        0x7A => Some('▫'),

        // Circled digits (Annex D.6, octal 254–323), previously dropped. Codes
        // are the spec's octal CODE in hex; each range is contiguous in Unicode. ~keep
        0xAC..=0xB5 => char::from_u32(0x2460 + (code as u32 - 0xAC)),
        0xB6..=0xBF => char::from_u32(0x2776 + (code as u32 - 0xB6)),
        0xC0..=0xC9 => char::from_u32(0x2780 + (code as u32 - 0xC0)),
        0xCA..=0xD3 => char::from_u32(0x278A + (code as u32 - 0xCA)),

        // Arrows (Annex D.6, octal 324–376): four singletons, then two runs. ~keep
        0xD4 => Some('\u{2794}'),
        0xD5 => Some('\u{2192}'),
        0xD6 => Some('\u{2194}'),
        0xD7 => Some('\u{2195}'),
        0xD8..=0xEF => char::from_u32(0x2798 + (code as u32 - 0xD8)),
        0xF1..=0xFE => char::from_u32(0x27B1 + (code as u32 - 0xF1)),

        _ => None,
    }
}

/// Look up a character in PDFDocEncoding.
///
/// PDFDocEncoding is a superset of ISO Latin-1 used as the default encoding
/// for PDF text strings and metadata (bookmarks, annotations, document info).
///
/// Codes 0-127 are identical to ASCII.
/// Codes 128-159 have special mappings (different from ISO Latin-1).
/// Codes 160-255 are identical to ISO Latin-1.
///
/// # PDF Spec Reference
///
/// ISO 32000-1:2008, Appendix D.2, Table D.2, page 994
///
/// # Arguments
///
/// * `code` - The byte code to look up (0-255)
///
/// # Returns
///
/// The Unicode character for this code, or None for undefined codes
pub fn pdfdoc_encoding_lookup(code: u8) -> Option<char> {
    match code {
        0x00..=0x7F => Some(code as char),

        0x80 => Some('•'),
        0x81 => Some('†'),
        0x82 => Some('‡'),
        0x83 => Some('…'),
        0x84 => Some('—'),
        0x85 => Some('–'),
        0x86 => Some('ƒ'),
        0x87 => Some('⁄'),
        0x88 => Some('‹'),
        0x89 => Some('›'),
        0x8A => Some('−'),
        0x8B => Some('‰'),
        0x8C => Some('„'),
        0x8D => Some('"'),
        0x8E => Some('"'),
        0x8F => Some('\u{2018}'),
        0x90 => Some('\u{2019}'),
        0x91 => Some('‚'),
        0x92 => Some('™'),
        0x93 => Some('ﬁ'),
        0x94 => Some('ﬂ'),
        0x95 => Some('Ł'),
        0x96 => Some('Œ'),
        0x97 => Some('Š'),
        0x98 => Some('Ÿ'),
        0x99 => Some('Ž'),
        0x9A => Some('ı'),
        0x9B => Some('ł'),
        0x9C => Some('œ'),
        0x9D => Some('š'),
        0x9E => Some('ž'),
        0x9F => None,

        0xA0..=0xFF => Some(code as char),
    }
}

/// Look up a character in a standard PDF encoding.
///
/// This function provides support for standard PDF encodings including
/// PDFDocEncoding, WinAnsiEncoding, StandardEncoding, and MacRomanEncoding.
///
/// # Arguments
///
/// * `encoding` - The encoding name (e.g., "WinAnsiEncoding", "PDFDocEncoding")
/// * `code` - The character code (0-255)
///
/// # Returns
///
/// Whether an embedded font program's built-in `/Encoding` (`prog_enc`,
/// code→char) looks like a re-indexed subset **cipher** rather than a
/// meaningful text encoding to overlay on the producer-declared named base
/// `std_name`.
///
/// A real encoding (a few non-standard slots over a named base, e.g. space at
/// 0xCA) agrees with the named base on most of the codes they share; a subset
/// cipher — the font's own arbitrary glyph ordering — agrees on almost none,
/// and overlaying it would rewrite every mapped code into mojibake. Decide by
/// agreement: of the codes present in both, fewer than half resolving to the
/// same character means cipher. Empty overlap is treated as NOT a cipher (no
/// evidence either way; keep the prior overlay behaviour).
fn builtin_encoding_looks_like_cipher(prog_enc: &HashMap<u8, char>, std_name: &str) -> bool {
    let (mut agree, mut overlap) = (0u32, 0u32);
    for (&code, &ch) in prog_enc {
        if let Some(us) = standard_encoding_lookup(std_name, code)
            && let Some(sc) = us.chars().next()
        {
            overlap += 1;
            if sc == ch {
                agree += 1;
            }
        }
    }
    overlap > 0 && (agree as f32 / overlap as f32) < 0.5
}

/// The Unicode string for this character, or None if not in the encoding.
fn standard_encoding_lookup(encoding: &str, code: u8) -> Option<String> {
    match encoding {
        "PDFDocEncoding" => pdfdoc_encoding_lookup(code).map(|c| c.to_string()),
        "WinAnsiEncoding" => {
            if (32..=126).contains(&code) {
                return Some((code as char).to_string());
            }

            // WinAnsiEncoding extended range (128-255)
            // Based on Windows-1252 encoding ~keep
            let unicode = match code {
                0x80 => '\u{20AC}',
                0x82 => '\u{201A}',
                0x83 => '\u{0192}',
                0x84 => '\u{201E}',
                0x85 => '\u{2026}',
                0x86 => '\u{2020}',
                0x87 => '\u{2021}',
                0x88 => '\u{02C6}',
                0x89 => '\u{2030}',
                0x8A => '\u{0160}',
                0x8B => '\u{2039}',
                0x8C => '\u{0152}',
                0x8E => '\u{017D}',
                0x91 => '\u{2018}',
                0x92 => '\u{2019}',
                0x93 => '\u{201C}',
                0x94 => '\u{201D}',
                0x95 => '\u{2022}',
                0x96 => '\u{2013}',
                0x97 => '\u{2014}',
                0x98 => '\u{02DC}',
                0x99 => '\u{2122}',
                0x9A => '\u{0161}',
                0x9B => '\u{203A}',
                0x9C => '\u{0153}',
                0x9E => '\u{017E}',
                0x9F => '\u{0178}',
                _ if code >= 0xA0 => char::from_u32(code as u32)?,
                _ => return None,
            };
            Some(unicode.to_string())
        }
        "StandardEncoding" => {
            // PostScript StandardEncoding per PDF Spec ISO 32000-1:2008, Annex D, Table D.1
            // NOTE: StandardEncoding differs significantly from ISO-8859-1 in the 0xA0-0xFF range.
            // Using ISO-8859-1 fallback here would produce wrong characters for ligatures,
            // smart quotes, accents, and other typographic characters. ~keep
            if (32..=126).contains(&code) {
                // Most codes in 32–126 match ASCII, with one notable exception:
                // 0x27 = "quoteright" → U+2019 (RIGHT SINGLE QUOTATION MARK)
                // All other printable ASCII codes are identity-mapped. ~keep
                let ch = match code {
                    0x27 => '\u{2019}',
                    _ => code as char,
                };
                Some(ch.to_string())
            } else {
                let unicode = match code {
                    0xA1 => '\u{00A1}',
                    0xA2 => '\u{00A2}',
                    0xA3 => '\u{00A3}',
                    0xA4 => '\u{2044}',
                    0xA5 => '\u{00A5}',
                    0xA6 => '\u{0192}',
                    0xA7 => '\u{00A7}',
                    0xA8 => '\u{00A4}',
                    0xA9 => '\u{0027}', // quotesingle (NOT copyright)
                    0xAA => '\u{201C}',
                    0xAB => '\u{00AB}',
                    0xAC => '\u{2039}',
                    0xAD => '\u{203A}',
                    0xAE => '\u{FB01}',
                    0xAF => '\u{FB02}',
                    0xB1 => '\u{2013}',
                    0xB2 => '\u{2020}',
                    0xB3 => '\u{2021}',
                    0xB4 => '\u{00B7}',
                    0xB6 => '\u{00B6}',
                    0xB7 => '\u{2022}',
                    0xB8 => '\u{201A}',
                    0xB9 => '\u{201E}',
                    0xBA => '\u{201D}',
                    0xBB => '\u{00BB}',
                    0xBC => '\u{2026}',
                    0xBD => '\u{2030}',
                    0xBF => '\u{00BF}',
                    0xC1 => '\u{0060}',
                    0xC2 => '\u{00B4}',
                    0xC3 => '\u{02C6}',
                    0xC4 => '\u{02DC}',
                    0xC5 => '\u{00AF}',
                    0xC6 => '\u{02D8}',
                    0xC7 => '\u{02D9}',
                    0xC8 => '\u{00A8}',
                    0xCA => '\u{02DA}',
                    0xCB => '\u{00B8}',
                    0xCD => '\u{02DD}',
                    0xCE => '\u{02DB}',
                    0xCF => '\u{02C7}',
                    0xD0 => '\u{2014}',
                    0xE1 => '\u{00C6}',
                    0xE3 => '\u{00AA}',
                    0xE8 => '\u{0141}',
                    0xE9 => '\u{00D8}',
                    0xEA => '\u{0152}',
                    0xEB => '\u{00BA}',
                    0xF1 => '\u{00E6}',
                    0xF5 => '\u{0131}',
                    0xF8 => '\u{0142}',
                    0xF9 => '\u{00F8}',
                    0xFA => '\u{0153}',
                    0xFB => '\u{00DF}',
                    _ => return None,
                };
                Some(unicode.to_string())
            }
        }
        "MacRomanEncoding" => {
            if (32..=126).contains(&code) {
                Some((code as char).to_string())
            } else {
                // Complete Mac OS Roman encoding per PDF Spec ISO 32000-1:2008, Annex D, Table D.2
                // ~keep
                let unicode = match code {
                    0x80 => '\u{00C4}',
                    0x81 => '\u{00C5}',
                    0x82 => '\u{00C7}',
                    0x83 => '\u{00C9}',
                    0x84 => '\u{00D1}',
                    0x85 => '\u{00D6}',
                    0x86 => '\u{00DC}',
                    0x87 => '\u{00E1}',
                    0x88 => '\u{00E0}',
                    0x89 => '\u{00E2}',
                    0x8A => '\u{00E4}',
                    0x8B => '\u{00E3}',
                    0x8C => '\u{00E5}',
                    0x8D => '\u{00E7}',
                    0x8E => '\u{00E9}',
                    0x8F => '\u{00E8}',
                    0x90 => '\u{00EA}',
                    0x91 => '\u{00EB}',
                    0x92 => '\u{00ED}',
                    0x93 => '\u{00EC}',
                    0x94 => '\u{00EE}',
                    0x95 => '\u{00EF}',
                    0x96 => '\u{00F1}',
                    0x97 => '\u{00F3}',
                    0x98 => '\u{00F2}',
                    0x99 => '\u{00F4}',
                    0x9A => '\u{00F6}',
                    0x9B => '\u{00F5}',
                    0x9C => '\u{00FA}',
                    0x9D => '\u{00F9}',
                    0x9E => '\u{00FB}',
                    0x9F => '\u{00FC}',
                    // 0xA0-0xBF: Symbols and punctuation (NOT Latin-1!) ~keep
                    0xA0 => '\u{2020}',
                    0xA1 => '\u{00B0}',
                    0xA2 => '\u{00A2}',
                    0xA3 => '\u{00A3}',
                    0xA4 => '\u{00A7}',
                    0xA5 => '\u{2022}',
                    0xA6 => '\u{00B6}',
                    0xA7 => '\u{00DF}',
                    0xA8 => '\u{00AE}',
                    0xA9 => '\u{00A9}', // copyright
                    0xAA => '\u{2122}',
                    0xAB => '\u{00B4}',
                    0xAC => '\u{00A8}',
                    0xAD => '\u{2260}',
                    0xAE => '\u{00C6}',
                    0xAF => '\u{00D8}',
                    0xB0 => '\u{221E}',
                    0xB1 => '\u{00B1}',
                    0xB2 => '\u{2264}',
                    0xB3 => '\u{2265}',
                    0xB4 => '\u{00A5}',
                    0xB5 => '\u{00B5}',
                    0xB6 => '\u{2202}',
                    0xB7 => '\u{2211}',
                    0xB8 => '\u{220F}',
                    0xB9 => '\u{03C0}',
                    0xBA => '\u{222B}',
                    0xBB => '\u{00AA}',
                    0xBC => '\u{00BA}',
                    0xBD => '\u{2126}',
                    0xBE => '\u{00E6}',
                    0xBF => '\u{00F8}',
                    0xC0 => '\u{00BF}',
                    0xC1 => '\u{00A1}',
                    0xC2 => '\u{00AC}',
                    0xC3 => '\u{221A}',
                    0xC4 => '\u{0192}',
                    0xC5 => '\u{2248}',
                    0xC6 => '\u{2206}',
                    0xC7 => '\u{00AB}',
                    0xC8 => '\u{00BB}',
                    0xC9 => '\u{2026}',
                    0xCA => '\u{00A0}',
                    0xCB => '\u{00C0}',
                    0xCC => '\u{00C3}',
                    0xCD => '\u{00D5}',
                    0xCE => '\u{0152}',
                    0xCF => '\u{0153}',
                    0xD0 => '\u{2013}',
                    0xD1 => '\u{2014}',
                    0xD2 => '\u{201C}',
                    0xD3 => '\u{201D}',
                    0xD4 => '\u{2018}',
                    0xD5 => '\u{2019}',
                    0xD6 => '\u{00F7}',
                    0xD7 => '\u{25CA}',
                    0xD8 => '\u{00FF}',
                    0xD9 => '\u{0178}',
                    0xDA => '\u{2044}',
                    0xDB => '\u{20AC}',
                    0xDC => '\u{2039}',
                    0xDD => '\u{203A}',
                    0xDE => '\u{FB01}',
                    0xDF => '\u{FB02}',
                    0xE0 => '\u{2021}',
                    0xE1 => '\u{00B7}',
                    0xE2 => '\u{201A}',
                    0xE3 => '\u{201E}',
                    0xE4 => '\u{2030}',
                    0xE5 => '\u{00C2}',
                    0xE6 => '\u{00CA}',
                    0xE7 => '\u{00C1}',
                    0xE8 => '\u{00CB}',
                    0xE9 => '\u{00C8}',
                    0xEA => '\u{00CD}',
                    0xEB => '\u{00CE}',
                    0xEC => '\u{00CF}',
                    0xED => '\u{00CC}',
                    0xEE => '\u{00D3}',
                    0xEF => '\u{00D4}',
                    0xF0 => '\u{F8FF}',
                    0xF1 => '\u{00D2}',
                    0xF2 => '\u{00DA}',
                    0xF3 => '\u{00DB}',
                    0xF4 => '\u{00D9}',
                    0xF5 => '\u{0131}',
                    0xF6 => '\u{02C6}',
                    0xF7 => '\u{02DC}',
                    0xF8 => '\u{00AF}',
                    0xF9 => '\u{02D8}',
                    0xFA => '\u{02D9}',
                    0xFB => '\u{02DA}',
                    0xFC => '\u{00B8}',
                    0xFD => '\u{02DD}',
                    0xFE => '\u{02DB}',
                    0xFF => '\u{02C7}',
                    _ => return None,
                };
                Some(unicode.to_string())
            }
        }
        _ => {
            if code.is_ascii() && code >= 32 {
                Some((code as char).to_string())
            } else {
                None
            }
        }
    }
}

/// Decode a raw CJK multi-byte character code to Unicode using legacy encodings.
///
/// For Type0 fonts using named CJK CMaps (e.g., "GBK-EUC-H", "GB-EUC-H",
/// "ETen-B5-H", "EUC-H", "KSC-EUC-H"), the 2-byte value read from the content
/// stream is NOT an Adobe CID — it is a raw multi-byte encoding value (GBK,
/// EUC-CN, Big5, EUC-JP, or EUC-KR). Adobe-GB1 CIDs cap at ~30 553, so
/// `lookup_predefined_cmap` always returns None for GBK values ≥ 0xA1A1,
/// the caller falls through to a broken `char::from_u32` path that maps them
/// to Korean Hangul (same code-point range).
///
/// This function catches that case and decodes with encoding_rs so the correct
/// CJK characters come out.
/// Selects the legacy multi-byte encoding for a non-Unicode-based predefined CMap name,
/// given the CIDSystemInfo ordering. Extracted verbatim from `decode_cjk_raw_charcode`'s
/// `enc` binding — pure code motion, same condition order and same `None` fallback. ~keep
fn select_cjk_legacy_encoding(enc_name: &str, ordering: &str) -> Option<&'static encoding_rs::Encoding> {
    if enc_name.contains("GBK")
        || enc_name.contains("GB-")
        || enc_name.contains("GBpc")
        || (enc_name.contains("EUC") && (ordering == "GB1" || enc_name.starts_with("GB")))
    {
        Some(encoding_rs::GBK)
    } else if enc_name.contains("B5") || enc_name.contains("CNS") || (enc_name.contains("EUC") && ordering == "CNS1") {
        Some(encoding_rs::BIG5)
    } else if enc_name.contains("EUC") && ordering == "Japan1" {
        Some(encoding_rs::EUC_JP)
    } else if (enc_name.contains("KSC") || enc_name.contains("KSCms")) && ordering == "Korea1" {
        Some(encoding_rs::EUC_KR)
    } else {
        None
    }
}

fn decode_cjk_raw_charcode(char_code: u32, enc_name: &str, cid_system_info: &Option<CIDSystemInfo>) -> Option<String> {
    let ordering = cid_system_info.as_ref().map(|i| i.ordering.as_str()).unwrap_or("");

    // CORPUS-3: the bare Adobe predefined CMaps "H"/"V" are (overwhelmingly)
    // Adobe-Japan1-H/V and carry JIS X 0208 codes in GL form (both bytes
    // 0x21–0x7E). encoding_rs decodes EUC-JP (high bit set), so lift GL→EUC by
    // OR-ing 0x8080, then decode. Recovers non-embedded Japanese (noembed-jis7:
    // "あいうえお" was emitted as garbage "CACCCECGCI"). ~keep
    if (enc_name == "H" || enc_name == "V") && (ordering == "Japan1" || ordering.is_empty()) {
        let hi = (char_code >> 8) & 0xFF;
        let lo = char_code & 0xFF;
        if (0x21..=0x7E).contains(&hi) && (0x21..=0x7E).contains(&lo) {
            let euc = [(hi | 0x80) as u8, (lo | 0x80) as u8];
            let (decoded, _, errors) = encoding_rs::EUC_JP.decode(&euc);
            if !errors {
                let r = decoded.replace('\u{FFFD}', "");
                if !r.is_empty() {
                    return Some(r);
                }
            }
        }
        if char_code <= 0x7E
            && let Some(c) = char::from_u32(char_code)
        {
            return Some(c.to_string());
        }
    }

    let enc = select_cjk_legacy_encoding(enc_name, ordering)?;

    // Reconstruct the raw bytes from the 2-byte char_code (big-endian) ~keep
    let bytes: [u8; 2] = [((char_code >> 8) & 0xFF) as u8, (char_code & 0xFF) as u8];

    let (decoded, _, errors) = enc.decode(&bytes);
    if errors {
        return None;
    }
    let result = decoded.replace('\u{FFFD}', "");
    if result.is_empty() { None } else { Some(result) }
}

// Maximum valid CID for each Adobe character collection (Fix C – OOB guard).
// CIDs beyond these values have no defined Unicode mapping; return None early
// to avoid accidental wrap-around in future table expansions.
//
// Sources:
//   Adobe-GB1-5 (TN #5079): 30,283 CIDs (0–30,283)
//   Adobe-Japan1-7 (TN #5078): 23,059 CIDs (0–23,059)
//   Adobe-CNS1-7 (TN #5080): 20,316 CIDs (0–20,316)
//   Adobe-Korea1-2 (TN #5093): 18,351 CIDs (0–18,351) ~keep
const CID_MAX_GB1: u16 = 30_283;
const CID_MAX_JAPAN1: u16 = 23_059;
const CID_MAX_CNS1: u16 = 20_316;
const CID_MAX_KOREA1: u16 = 18_351;

/// Lookup Unicode code point for a CID in a predefined Unicode-based CMap.
///
/// Predefined CMaps for CJK fonts map CID values from Adobe character collections to Unicode.
/// Per PDF Spec ISO 32000-1:2008 Section 9.7.5.2.
///
/// # Arguments
///
/// * `cmap_name` - The predefined CMap name (e.g., "UniGB-UCS2-H")
/// * `cid_system_info` - The CIDSystemInfo identifying the character collection
/// * `cid` - The Character ID (CID) to look up
///
/// # Returns
///
/// The corresponding Unicode code point, or None if not found.
///
/// # Predefined CMaps Supported
///
/// - UniGB-UCS2-H: Adobe-GB1 (Simplified Chinese)
/// - UniJIS-UCS2-H: Adobe-Japan1 (Japanese)
/// - UniCNS-UCS2-H: Adobe-CNS1 (Traditional Chinese)
/// - UniKS-UCS2-H: Adobe-Korea1 (Korean)
fn lookup_predefined_cmap(cmap_name: &str, cid_system_info: &Option<CIDSystemInfo>, cid: u16) -> Option<u32> {
    let system_info = cid_system_info.as_ref()?;

    // Fix C: guard out-of-bounds CIDs before hitting the lookup table.
    // CIDs beyond the collection maximum have no defined Unicode mapping. ~keep
    let max_cid = match system_info.ordering.as_str() {
        "GB1" => CID_MAX_GB1,
        "Japan1" => CID_MAX_JAPAN1,
        "CNS1" => CID_MAX_CNS1,
        "Korea1" => CID_MAX_KOREA1,
        // Adobe-Arabic-1 / Adobe-Persian-1: `lookup_adobe_arabic` rejects
        // unmapped CIDs itself, so the bound is just an early-out. ~keep
        "Arabic" | "Persian" => u16::MAX,
        _ => return None,
    };
    if cid > max_cid {
        tracing::trace!(
            "CID {} exceeds max {} for ordering '{}' → returning None (OOB)",
            cid,
            max_cid,
            system_info.ordering
        );
        return None;
    }

    match (cmap_name, system_info.ordering.as_str()) {
        ("UniGB-UCS2-H", "GB1") => lookup_adobe_gb1_to_unicode(cid),
        ("UniJIS-UCS2-H", "Japan1") => lookup_adobe_japan1_to_unicode(cid),
        ("UniCNS-UCS2-H", "CNS1") => lookup_adobe_cns1_to_unicode(cid),
        ("UniKS-UCS2-H", "Korea1") => lookup_adobe_korea1_to_unicode(cid),
        // Fallback: match by CIDSystemInfo ordering alone.
        // Some PDFs use encoding CMaps with custom names (e.g., "Adobe-Japan1-2")
        // that are identity mappings (charcode == CID). The CID→Unicode lookup
        // should still work based on the character collection ordering. ~keep
        (_, "GB1") => lookup_adobe_gb1_to_unicode(cid),
        (_, "Japan1") => lookup_adobe_japan1_to_unicode(cid),
        (_, "CNS1") => lookup_adobe_cns1_to_unicode(cid),
        (_, "Korea1") => lookup_adobe_korea1_to_unicode(cid),
        // Adobe-Arabic-1 / Adobe-Persian-1 CIDFonts without /ToUnicode (Nazanin,
        // Yagut, Mitra, Lotus). `lookup_adobe_arabic` is the §9.10.3 step-3
        // identity fallback; without it these decode as Latin-Extended-B garbage. ~keep
        (_, "Arabic") | (_, "Persian") => crate::fonts::cid_mappings::lookup_adobe_arabic(cid),
        _ => None,
    }
}

/// Map CID from Adobe-GB1 character collection to Unicode.
///
/// Adobe-GB1 contains Simplified Chinese characters from GB 2312 and extensions.
/// Reference: Adobe Technical Note #5079 (Adobe-GB1-4 Character Collection)
fn lookup_adobe_gb1_to_unicode(cid: u16) -> Option<u32> {
    crate::fonts::cid_mappings::lookup_adobe_gb1(cid)
}

/// Map CID from Adobe-Japan1 character collection to Unicode.
///
/// Adobe-Japan1 contains Japanese characters from JIS X 0208, JIS X 0212, etc.
/// Reference: Adobe Technical Note #5078 (Adobe-Japan1-4 Character Collection)
fn lookup_adobe_japan1_to_unicode(cid: u16) -> Option<u32> {
    crate::fonts::cid_mappings::lookup_adobe_japan1(cid)
}

/// Map CID from Adobe-CNS1 character collection to Unicode.
///
/// Adobe-CNS1 contains Traditional Chinese characters from CNS 11643 and extensions.
/// Reference: Adobe Technical Note #5080 (Adobe-CNS1-4 Character Collection)
fn lookup_adobe_cns1_to_unicode(cid: u16) -> Option<u32> {
    crate::fonts::cid_mappings::lookup_adobe_cns1(cid)
}

/// Map CID from Adobe-Korea1 character collection to Unicode.
///
/// Adobe-Korea1 contains Korean characters from KS X 1001 and KS X 1002.
/// Reference: Adobe Technical Note #5093 (Adobe-Korea1-2 Character Collection)
fn lookup_adobe_korea1_to_unicode(cid: u16) -> Option<u32> {
    crate::fonts::cid_mappings::lookup_adobe_korea1(cid)
}

/// Ascent/descent (as fractions of em) for the 14 standard PDF fonts.
/// Values from Adobe AFM files; used when no FontDescriptor is present.
fn standard_font_metrics(base_font: &str) -> Option<(f32, f32)> {
    // Strip subset prefix (e.g. "ABCDEF+Courier" -> "Courier") ~keep
    let name = if let Some(pos) = base_font.find('+') {
        &base_font[pos + 1..]
    } else {
        base_font
    };
    match name {
        "Courier" | "Courier-Bold" | "Courier-Oblique" | "Courier-BoldOblique" => Some((0.629, -0.157)),
        "Helvetica" | "Helvetica-Bold" | "Helvetica-Oblique" | "Helvetica-BoldOblique" => Some((0.718, -0.207)),
        "Times-Roman" => Some((0.683, -0.217)),
        "Times-Bold" => Some((0.676, -0.205)),
        "Times-Italic" => Some((0.683, -0.205)),
        "Times-BoldItalic" => Some((0.683, -0.205)),
        "Symbol" => Some((1.010, -0.293)),
        "ZapfDingbats" => Some((0.820, -0.143)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cipher_discriminator_flags_disagreeing_builtin_encoding() {
        // A subset cipher: ASCII-letter codes resolve to unrelated glyphs, so
        // they disagree with WinAnsi on every overlapping code (0/N agree). ~keep
        let cipher: HashMap<u8, char> = [(b'A', 'ñ'), (b'B', 'k'), (b'C', 'º'), (b'D', 'p')]
            .into_iter()
            .collect();
        assert!(builtin_encoding_looks_like_cipher(&cipher, "WinAnsiEncoding"));
    }

    #[test]
    fn cipher_discriminator_keeps_mostly_agreeing_builtin_encoding() {
        // A real text encoding: agrees with the named base on most codes (a
        // single non-standard slot is not enough to look like a cipher). ~keep
        let real: HashMap<u8, char> = [(b'A', 'A'), (b'B', 'B'), (b'C', 'C'), (0xCA, ' ')]
            .into_iter()
            .collect();
        assert!(!builtin_encoding_looks_like_cipher(&real, "WinAnsiEncoding"));
    }

    #[test]
    fn cipher_discriminator_no_overlap_is_not_a_cipher() {
        // No codes overlap the named base's mapped range → no evidence → not a
        // cipher (preserve the prior overlay behaviour). ~keep
        let empty: HashMap<u8, char> = HashMap::new();
        assert!(!builtin_encoding_looks_like_cipher(&empty, "WinAnsiEncoding"));
    }

    #[test]
    fn test_standard_encoding_ascii() {
        assert_eq!(standard_encoding_lookup("WinAnsiEncoding", b'A'), Some("A".to_string()));
        assert_eq!(standard_encoding_lookup("WinAnsiEncoding", b'Z'), Some("Z".to_string()));
        assert_eq!(standard_encoding_lookup("WinAnsiEncoding", b'0'), Some("0".to_string()));
    }

    #[test]
    fn test_standard_encoding_space() {
        assert_eq!(standard_encoding_lookup("WinAnsiEncoding", b' '), Some(" ".to_string()));
    }

    #[test]
    fn test_font_info_is_bold() {
        let font = FontInfo {
            base_font: "Times-Bold".to_string(),
            subtype: "Type1".to_string(),
            encoding: Encoding::Standard("WinAnsiEncoding".to_string()),
            to_unicode: None,
            font_weight: Some(700),
            flags: None,
            stem_v: None,
            ascent: 0.95,
            descent: -0.35,
            embedded_font_data: None,
            truetype_cmap: std::sync::OnceLock::new(),
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font: false,
            widths: None,
            first_char: None,
            last_char: None,
            font_matrix_a: 0.001,
            default_width: 1000.0,
            cid_to_gid_map: None,
            cid_system_info: None,
            cid_font_type: None,
            cid_widths: None,
            cid_default_width: 1000.0,
            has_explicit_dw: false,
            cff_gid_map: None,
            multi_char_map: HashMap::new(),
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names: std::collections::HashMap::new(),
            wmode: 0,
            cid_vertical_metrics: None,
            cid_default_vertical_metrics: VerticalMetrics::SPEC_DEFAULT,
            cjk_substitution: None,
        };
        assert!(font.is_bold());

        let font2 = FontInfo {
            base_font: "Helvetica".to_string(),
            subtype: "Type1".to_string(),
            encoding: Encoding::Standard("WinAnsiEncoding".to_string()),
            to_unicode: None,
            font_weight: Some(400),
            flags: None,
            stem_v: None,
            ascent: 0.95,
            descent: -0.35,
            embedded_font_data: None,
            truetype_cmap: std::sync::OnceLock::new(),
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font: false,
            widths: None,
            first_char: None,
            last_char: None,
            font_matrix_a: 0.001,
            default_width: 1000.0,
            cid_to_gid_map: None,
            cid_system_info: None,
            cid_font_type: None,
            cid_widths: None,
            cid_default_width: 1000.0,
            has_explicit_dw: false,
            cff_gid_map: None,
            multi_char_map: HashMap::new(),
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names: std::collections::HashMap::new(),
            wmode: 0,
            cid_vertical_metrics: None,
            cid_default_vertical_metrics: VerticalMetrics::SPEC_DEFAULT,
            cjk_substitution: None,
        };
        assert!(!font2.is_bold());
    }

    #[test]
    fn test_font_info_is_italic() {
        let font = FontInfo {
            base_font: "Times-Italic".to_string(),
            subtype: "Type1".to_string(),
            encoding: Encoding::Standard("WinAnsiEncoding".to_string()),
            to_unicode: None,
            font_weight: None,
            flags: None,
            stem_v: None,
            ascent: 0.95,
            descent: -0.35,
            embedded_font_data: None,
            truetype_cmap: std::sync::OnceLock::new(),
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font: false,
            widths: None,
            first_char: None,
            last_char: None,
            font_matrix_a: 0.001,
            default_width: 1000.0,
            cid_to_gid_map: None,
            cid_system_info: None,
            cid_font_type: None,
            cid_widths: None,
            cid_default_width: 1000.0,
            has_explicit_dw: false,
            cff_gid_map: None,
            multi_char_map: HashMap::new(),
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names: std::collections::HashMap::new(),
            wmode: 0,
            cid_vertical_metrics: None,
            cid_default_vertical_metrics: VerticalMetrics::SPEC_DEFAULT,
            cjk_substitution: None,
        };
        assert!(font.is_italic());

        let font2 = FontInfo {
            base_font: "Courier-Oblique".to_string(),
            subtype: "Type1".to_string(),
            encoding: Encoding::Standard("WinAnsiEncoding".to_string()),
            to_unicode: None,
            font_weight: None,
            flags: None,
            stem_v: None,
            ascent: 0.95,
            descent: -0.35,
            embedded_font_data: None,
            truetype_cmap: std::sync::OnceLock::new(),
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font: false,
            widths: None,
            first_char: None,
            last_char: None,
            font_matrix_a: 0.001,
            default_width: 1000.0,
            cid_to_gid_map: None,
            cid_system_info: None,
            cid_font_type: None,
            cid_widths: None,
            cid_default_width: 1000.0,
            has_explicit_dw: false,
            cff_gid_map: None,
            multi_char_map: HashMap::new(),
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names: std::collections::HashMap::new(),
            wmode: 0,
            cid_vertical_metrics: None,
            cid_default_vertical_metrics: VerticalMetrics::SPEC_DEFAULT,
            cjk_substitution: None,
        };
        assert!(font2.is_italic());
    }

    #[test]
    fn test_char_to_unicode_with_tounicode() {
        let cmap_data = b"beginbfchar\n<0041> <0058>\nendbfchar";

        let font = FontInfo {
            base_font: "CustomFont".to_string(),
            subtype: "Type1".to_string(),
            encoding: Encoding::Standard("WinAnsiEncoding".to_string()),
            to_unicode: Some(LazyCMap::new(cmap_data.to_vec())),
            font_weight: None,
            flags: None,
            stem_v: None,
            ascent: 0.95,
            descent: -0.35,
            embedded_font_data: None,
            truetype_cmap: std::sync::OnceLock::new(),
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font: false,
            widths: None,
            first_char: None,
            last_char: None,
            font_matrix_a: 0.001,
            default_width: 1000.0,
            cid_to_gid_map: None,
            cid_system_info: None,
            cid_font_type: None,
            cid_widths: None,
            cid_default_width: 1000.0,
            has_explicit_dw: false,
            cff_gid_map: None,
            multi_char_map: HashMap::new(),
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names: std::collections::HashMap::new(),
            wmode: 0,
            cid_vertical_metrics: None,
            cid_default_vertical_metrics: VerticalMetrics::SPEC_DEFAULT,
            cjk_substitution: None,
        };

        assert_eq!(font.char_to_unicode(0x41), Some("X".to_string()));
        assert_eq!(font.char_to_unicode(0x42), Some("B".to_string()));
    }

    // ------------------------------------------------------------------
    // `capture_warnings` installs a minimal in-process `tracing::Subscriber`
    // that records event messages, scoped only to the closure passed to it.
    // Mirrors `RecordingSubscriber` in `src/fonts/cmap.rs`; duplicated here
    // rather than shared because that one is private to cmap.rs's own test
    // module and this change may only touch this file. ~keep
    // ------------------------------------------------------------------

    #[derive(Clone, Default)]
    struct RecordingSubscriber {
        messages: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }

    #[derive(Default)]
    struct MessageVisitor(Vec<String>);

    impl tracing::field::Visit for MessageVisitor {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.0.push(format!("{}={value:?}", field.name()));
        }
    }

    impl tracing::Subscriber for RecordingSubscriber {
        fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }

        fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

        fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

        fn event(&self, event: &tracing::Event<'_>) {
            if *event.metadata().level() != tracing::Level::WARN {
                return;
            }
            let mut visitor = MessageVisitor::default();
            event.record(&mut visitor);
            if let Ok(mut messages) = self.messages.lock() {
                messages.push(visitor.0.join(" "));
            }
        }

        fn enter(&self, _span: &tracing::span::Id) {}

        fn exit(&self, _span: &tracing::span::Id) {}
    }

    /// Run `f` under a subscriber that records every tracing event message,
    /// returning them in emission order.
    fn capture_warnings<F: FnOnce()>(f: F) -> Vec<String> {
        let subscriber = RecordingSubscriber::default();
        let messages = std::sync::Arc::clone(&subscriber.messages);
        tracing::subscriber::with_default(subscriber, f);
        messages.lock().map(|guard| guard.clone()).unwrap_or_default()
    }

    #[test]
    fn font_dictionary_warnings_exclude_untrusted_names_and_objects() {
        const SECRET_FONT: &str = "CONFIDENTIAL_FONT_73da";
        const SECRET_OBJECT: &str = "CONFIDENTIAL_DIFFERENCE_916c";
        let doc = minimal_pdf_doc();
        let type3 = Object::Dictionary(HashMap::from([
            ("Subtype".to_string(), Object::Name("Type3".to_string())),
            ("BaseFont".to_string(), Object::Name(SECRET_FONT.to_string())),
        ]));
        let encoding = Object::Dictionary(HashMap::from([(
            "Differences".to_string(),
            Object::Array(vec![Object::String(SECRET_OBJECT.as_bytes().to_vec())]),
        )]));

        crate::extractors::warnings::drain_global_warnings();
        let logs = capture_warnings(|| {
            FontInfo::from_dict(&type3, &doc).expect("minimal Type3 dictionary must parse");
            FontInfo::parse_encoding(&encoding, &doc, None).expect("malformed differences must recover");
        });
        crate::extractors::warnings::drain_global_warnings();

        assert_eq!(logs.len(), 2, "expected exactly two recovery warnings: {logs:#?}");
        assert_eq!(
            logs.iter()
                .filter(|event| {
                    event.contains("operation=\"load_font\"") && event.contains("error_code=\"type3_font\"")
                })
                .count(),
            1
        );
        assert_eq!(
            logs.iter()
                .filter(|event| {
                    event.contains("operation=\"parse_font_encoding\"")
                        && event.contains("error_code=\"invalid_differences_entry\"")
                })
                .count(),
            1
        );
        let rendered = format!("{logs:?}");
        assert!(!rendered.contains(SECRET_FONT));
        assert!(!rendered.contains(SECRET_OBJECT));
    }

    #[test]
    fn cid_font_recovery_warnings_cover_all_malformed_branches_without_names() {
        const SECRET_FONT: &str = "CONFIDENTIAL_CID_FONT_01c8";
        const SECRET_OBJECT: &str = "CONFIDENTIAL_CID_OBJECT_79e3";
        let doc = minimal_pdf_doc();
        let system_info = Object::Dictionary(HashMap::from([
            ("Registry".to_string(), Object::String(b"Adobe".to_vec())),
            ("Ordering".to_string(), Object::String(b"Identity".to_vec())),
            ("Supplement".to_string(), Object::Integer(0)),
        ]));
        let cid_font = Object::Dictionary(HashMap::from([
            ("Subtype".to_string(), Object::Name("CIDFontType2".to_string())),
            ("CIDSystemInfo".to_string(), system_info),
            (
                "CIDToGIDMap".to_string(),
                Object::String(SECRET_OBJECT.as_bytes().to_vec()),
            ),
        ]));
        let descendants = HashMap::from([(
            "DescendantFonts".to_string(),
            Object::Array(vec![cid_font, Object::Null]),
        )]);

        let logs = capture_warnings(|| {
            FontInfo::parse_descendant_fonts(&descendants, SECRET_FONT, &doc).unwrap();
            for w2 in [
                vec![Object::Name(SECRET_OBJECT.to_string())],
                vec![
                    Object::Integer(65_535),
                    Object::Array(vec![
                        Object::Integer(1),
                        Object::Integer(2),
                        Object::Integer(3),
                        Object::Integer(4),
                        Object::Integer(5),
                        Object::Integer(6),
                    ]),
                ],
                vec![
                    Object::Integer(1),
                    Object::Array(vec![
                        Object::Name(SECRET_OBJECT.to_string()),
                        Object::Integer(2),
                        Object::Integer(3),
                    ]),
                ],
                vec![Object::Integer(1), Object::Integer(2), Object::Integer(3)],
                vec![Object::Integer(1), Object::Name(SECRET_OBJECT.to_string())],
            ] {
                FontInfo::parse_cid_vertical_metrics(
                    &HashMap::from([("W2".to_string(), Object::Array(w2))]),
                    SECRET_FONT,
                );
            }
            for widths in [
                vec![Object::Name(SECRET_OBJECT.to_string())],
                vec![Object::Integer(1), Object::Integer(2)],
                vec![
                    Object::Integer(1),
                    Object::Integer(2),
                    Object::Name(SECRET_OBJECT.to_string()),
                ],
                vec![Object::Integer(1), Object::Name(SECRET_OBJECT.to_string())],
            ] {
                FontInfo::parse_cid_widths(&HashMap::from([("W".to_string(), Object::Array(widths))]), SECRET_FONT);
            }
        });

        assert_eq!(logs.len(), 12, "expected every malformed CID branch once: {logs:#?}");
        for (operation, error_code) in [
            ("parse_descendant_fonts", "extra_descendants"),
            ("parse_descendant_fonts", "inline_descendant"),
            ("parse_cid_to_gid_map", "invalid_map_type"),
            ("parse_cid_vertical_metrics", "invalid_start_cid"),
            ("parse_cid_vertical_metrics", "cid_out_of_range"),
            ("parse_cid_vertical_metrics", "invalid_metric_triple"),
            ("parse_cid_vertical_metrics", "truncated_range"),
            ("parse_cid_vertical_metrics", "invalid_range_type"),
            ("parse_cid_widths", "invalid_start_cid"),
            ("parse_cid_widths", "missing_range_width"),
            ("parse_cid_widths", "invalid_range_width"),
            ("parse_cid_widths", "invalid_range_type"),
        ] {
            assert_eq!(
                logs.iter()
                    .filter(|event| {
                        event.contains(&format!("operation=\"{operation}\""))
                            && event.contains(&format!("error_code=\"{error_code}\""))
                    })
                    .count(),
                1,
                "missing exact {operation}/{error_code} event: {logs:#?}"
            );
        }
        let rendered = format!("{logs:?}");
        assert!(!rendered.contains(SECRET_FONT));
        assert!(!rendered.contains(SECRET_OBJECT));
    }

    /// A font whose /ToUnicode CMap fails to parse must WARN about the
    /// fallback exactly once, no matter how many distinct character codes
    /// are decoded — this branch has no `OnceLock` of its own, so the
    /// guard reuses `type0_unicode_memo` (see the `~keep` comment on the
    /// `else` arm in `char_to_unicode_uncached`) instead of re-parsing per
    /// glyph and spamming one WARN per code.
    #[test]
    fn unparseable_tounicode_cmap_warns_once_across_many_character_codes() {
        let garbage_cmap = b"RANDOM BINARY GARBAGE, NOT A CMAP STREAM AT ALL 0xDEADBEEF".to_vec();

        let font = FontInfo {
            base_font: "BrokenCMapFont".to_string(),
            subtype: "Type0".to_string(),
            encoding: Encoding::Identity,
            to_unicode: Some(LazyCMap::new_for_font(garbage_cmap, "BrokenCMapFont".to_string())),
            font_weight: None,
            flags: None,
            stem_v: None,
            ascent: 0.95,
            descent: -0.35,
            embedded_font_data: None,
            truetype_cmap: std::sync::OnceLock::new(),
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font: false,
            widths: None,
            first_char: None,
            last_char: None,
            font_matrix_a: 0.001,
            default_width: 1000.0,
            cid_to_gid_map: None,
            cid_system_info: None,
            cid_font_type: Some("CIDFontType2".to_string()),
            cid_widths: None,
            cid_default_width: 1000.0,
            has_explicit_dw: false,
            cff_gid_map: None,
            multi_char_map: HashMap::new(),
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names: std::collections::HashMap::new(),
            wmode: 0,
            cid_vertical_metrics: None,
            cid_default_vertical_metrics: VerticalMetrics::SPEC_DEFAULT,
            cjk_substitution: None,
        };

        let logs = capture_warnings(|| {
            for char_code in 0u32..50 {
                let _ = font.char_to_unicode(char_code);
            }
        });

        let failure_warnings = logs
            .iter()
            .filter(|message| message.contains("PDF operation degraded"))
            .count();
        assert_eq!(
            failure_warnings, 1,
            "a broken ToUnicode must WARN once per font, not once per character \
             code and not twice for one failure; got: {logs:?}"
        );
    }

    #[test]
    fn test_char_to_unicode_standard_encoding() {
        let font = FontInfo {
            base_font: "Times-Roman".to_string(),
            subtype: "Type1".to_string(),
            encoding: Encoding::Standard("WinAnsiEncoding".to_string()),
            to_unicode: None,
            font_weight: None,
            flags: None,
            stem_v: None,
            ascent: 0.95,
            descent: -0.35,
            embedded_font_data: None,
            truetype_cmap: std::sync::OnceLock::new(),
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font: false,
            widths: None,
            first_char: None,
            last_char: None,
            font_matrix_a: 0.001,
            default_width: 1000.0,
            cid_to_gid_map: None,
            cid_system_info: None,
            cid_font_type: None,
            cid_widths: None,
            cid_default_width: 1000.0,
            has_explicit_dw: false,
            cff_gid_map: None,
            multi_char_map: HashMap::new(),
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names: std::collections::HashMap::new(),
            wmode: 0,
            cid_vertical_metrics: None,
            cid_default_vertical_metrics: VerticalMetrics::SPEC_DEFAULT,
            cjk_substitution: None,
        };

        assert_eq!(font.char_to_unicode(0x41), Some("A".to_string()));
        assert_eq!(font.char_to_unicode(0x20), Some(" ".to_string()));
    }

    #[test]
    fn test_char_to_unicode_identity() {
        let font_type0 = FontInfo {
            base_font: "CIDFont".to_string(),
            subtype: "Type0".to_string(),
            encoding: Encoding::Identity,
            to_unicode: None,
            font_weight: None,
            flags: None,
            stem_v: None,
            ascent: 0.95,
            descent: -0.35,
            embedded_font_data: None,
            truetype_cmap: std::sync::OnceLock::new(),
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font: false,
            widths: None,
            first_char: None,
            last_char: None,
            font_matrix_a: 0.001,
            default_width: 1000.0,
            cid_to_gid_map: None,
            cid_system_info: None,
            cid_font_type: None,
            cid_widths: None,
            cid_default_width: 1000.0,
            has_explicit_dw: false,
            cff_gid_map: None,
            multi_char_map: HashMap::new(),
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names: std::collections::HashMap::new(),
            wmode: 0,
            cid_vertical_metrics: None,
            cid_default_vertical_metrics: VerticalMetrics::SPEC_DEFAULT,
            cjk_substitution: None,
        };

        assert_eq!(font_type0.char_to_unicode(0x41), Some("A".to_string()));
        assert_eq!(font_type0.char_to_unicode(0x263A), Some("\u{263A}".to_string()));

        let font_type1 = FontInfo {
            base_font: "TimesRoman".to_string(),
            subtype: "Type1".to_string(),
            encoding: Encoding::Identity,
            to_unicode: None,
            font_weight: None,
            flags: None,
            stem_v: None,
            ascent: 0.95,
            descent: -0.35,
            embedded_font_data: None,
            truetype_cmap: std::sync::OnceLock::new(),
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font: false,
            widths: None,
            first_char: None,
            last_char: None,
            font_matrix_a: 0.001,
            default_width: 1000.0,
            cid_to_gid_map: None,
            cid_system_info: None,
            cid_font_type: None,
            cid_widths: None,
            cid_default_width: 1000.0,
            has_explicit_dw: false,
            cff_gid_map: None,
            multi_char_map: HashMap::new(),
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names: std::collections::HashMap::new(),
            wmode: 0,
            cid_vertical_metrics: None,
            cid_default_vertical_metrics: VerticalMetrics::SPEC_DEFAULT,
            cjk_substitution: None,
        };

        assert_eq!(font_type1.char_to_unicode(0x41), Some("A".to_string()));
        assert_eq!(font_type1.char_to_unicode(0x263A), Some("☺".to_string()));
    }

    #[test]
    fn test_lookup_predefined_cmap_adobe_gb1() {
        let cid_system_info = Some(CIDSystemInfo {
            registry: "Adobe".to_string(),
            ordering: "GB1".to_string(),
            supplement: 2,
        });

        assert_eq!(lookup_predefined_cmap("UniGB-UCS2-H", &cid_system_info, 34), Some(0x41));

        assert_eq!(
            lookup_predefined_cmap("UniGB-UCS2-H", &cid_system_info, 4559),
            Some(0x4E2D)
        );

        assert_eq!(lookup_predefined_cmap("UniGB-UCS2-H", &cid_system_info, 50000), None);

        assert_eq!(lookup_predefined_cmap("UniGB-UCS2-H", &None, 34), None);
    }

    #[test]
    fn test_lookup_predefined_cmap_adobe_japan1() {
        let cid_system_info = Some(CIDSystemInfo {
            registry: "Adobe".to_string(),
            ordering: "Japan1".to_string(),
            supplement: 4,
        });

        assert_eq!(
            lookup_predefined_cmap("UniJIS-UCS2-H", &cid_system_info, 34),
            Some(0x41)
        );

        assert_eq!(
            lookup_predefined_cmap("UniJIS-UCS2-H", &cid_system_info, 843),
            Some(0x3042)
        );

        assert_eq!(lookup_predefined_cmap("UniJIS-UCS2-H", &cid_system_info, 50000), None);
    }

    #[test]
    fn test_lookup_predefined_cmap_adobe_cns1() {
        let cid_system_info = Some(CIDSystemInfo {
            registry: "Adobe".to_string(),
            ordering: "CNS1".to_string(),
            supplement: 3,
        });

        assert_eq!(
            lookup_predefined_cmap("UniCNS-UCS2-H", &cid_system_info, 34),
            Some(0x41)
        );

        assert_eq!(
            lookup_predefined_cmap("UniCNS-UCS2-H", &cid_system_info, 595),
            Some(0x4E00)
        );
    }

    #[test]
    fn test_lookup_predefined_cmap_adobe_korea1() {
        let cid_system_info = Some(CIDSystemInfo {
            registry: "Adobe".to_string(),
            ordering: "Korea1".to_string(),
            supplement: 1,
        });

        assert_eq!(lookup_predefined_cmap("UniKS-UCS2-H", &cid_system_info, 34), Some(0x41));

        assert_eq!(
            lookup_predefined_cmap("UniKS-UCS2-H", &cid_system_info, 1086),
            Some(0xAC00)
        );
    }

    #[test]
    fn test_lookup_predefined_cmap_wrong_ordering() {
        let cid_system_info_wrong = Some(CIDSystemInfo {
            registry: "Adobe".to_string(),
            ordering: "WrongOrdering".to_string(),
            supplement: 1,
        });

        assert_eq!(lookup_predefined_cmap("UniGB-UCS2-H", &cid_system_info_wrong, 34), None);
    }

    #[test]
    fn test_encoding_clone() {
        let enc = Encoding::Standard("WinAnsiEncoding".to_string());
        let enc2 = enc.clone();
        match enc2 {
            Encoding::Standard(name) => assert_eq!(name, "WinAnsiEncoding"),
            _ => panic!("Wrong encoding type"),
        }
    }

    #[test]
    fn test_font_info_clone() {
        let font = FontInfo {
            base_font: "Test".to_string(),
            subtype: "Type1".to_string(),
            encoding: Encoding::Standard("WinAnsiEncoding".to_string()),
            to_unicode: None,
            font_weight: None,
            flags: None,
            stem_v: None,
            ascent: 0.95,
            descent: -0.35,
            embedded_font_data: None,
            truetype_cmap: std::sync::OnceLock::new(),
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font: false,
            widths: None,
            first_char: None,
            last_char: None,
            font_matrix_a: 0.001,
            default_width: 1000.0,
            cid_to_gid_map: None,
            cid_system_info: None,
            cid_font_type: None,
            cid_widths: None,
            cid_default_width: 1000.0,
            has_explicit_dw: false,
            cff_gid_map: None,
            multi_char_map: HashMap::new(),
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names: std::collections::HashMap::new(),
            wmode: 0,
            cid_vertical_metrics: None,
            cid_default_vertical_metrics: VerticalMetrics::SPEC_DEFAULT,
            cjk_substitution: None,
        };

        let font2 = font.clone();
        assert_eq!(font2.base_font, "Test");
    }

    #[test]
    fn test_glyph_name_to_unicode_basic() {
        assert_eq!(glyph_name_to_unicode("A"), Some('A'));
        assert_eq!(glyph_name_to_unicode("a"), Some('a'));
        assert_eq!(glyph_name_to_unicode("zero"), Some('0'));
        assert_eq!(glyph_name_to_unicode("nine"), Some('9'));
    }

    #[test]
    fn test_glyph_name_to_unicode_punctuation() {
        assert_eq!(glyph_name_to_unicode("space"), Some(' '));
        assert_eq!(glyph_name_to_unicode("quotesingle"), Some('\''));
        assert_eq!(glyph_name_to_unicode("grave"), Some('`'));
        assert_eq!(glyph_name_to_unicode("hyphen"), Some('-'));
        // Official AGL: "minus" maps to U+2212 (MINUS SIGN), not U+002D (HYPHEN-MINUS) ~keep
        assert_eq!(glyph_name_to_unicode("minus"), Some('−'));
    }

    #[test]
    fn test_glyph_name_to_unicode_special() {
        assert_eq!(glyph_name_to_unicode("bullet"), Some('•'));
        assert_eq!(glyph_name_to_unicode("dagger"), Some('†'));
        assert_eq!(glyph_name_to_unicode("daggerdbl"), Some('‡'));
        assert_eq!(glyph_name_to_unicode("ellipsis"), Some('…'));
        assert_eq!(glyph_name_to_unicode("emdash"), Some('—'));
        assert_eq!(glyph_name_to_unicode("endash"), Some('–'));
    }

    #[test]
    fn test_glyph_name_to_unicode_quotes() {
        assert_eq!(glyph_name_to_unicode("quotesinglbase"), Some('‚'));
        assert_eq!(glyph_name_to_unicode("quotedblbase"), Some('„'));
        // Official AGL uses proper curly quotes, not straight quotes ~keep
        assert_eq!(glyph_name_to_unicode("quotedblleft"), Some('\u{201C}'));
        assert_eq!(glyph_name_to_unicode("quotedblright"), Some('\u{201D}'));
        assert_eq!(glyph_name_to_unicode("quoteleft"), Some('\u{2018}'));
        assert_eq!(glyph_name_to_unicode("quoteright"), Some('\u{2019}'));
    }

    #[test]
    fn test_glyph_name_to_unicode_accented() {
        assert_eq!(glyph_name_to_unicode("Aacute"), Some('Á'));
        assert_eq!(glyph_name_to_unicode("aacute"), Some('á'));
        assert_eq!(glyph_name_to_unicode("Ntilde"), Some('Ñ'));
        assert_eq!(glyph_name_to_unicode("ntilde"), Some('ñ'));
    }

    #[test]
    fn test_glyph_name_to_unicode_currency() {
        assert_eq!(glyph_name_to_unicode("Euro"), Some('€'));
        assert_eq!(glyph_name_to_unicode("sterling"), Some('£'));
        assert_eq!(glyph_name_to_unicode("yen"), Some('¥'));
        assert_eq!(glyph_name_to_unicode("cent"), Some('¢'));
    }

    #[test]
    fn test_glyph_name_to_unicode_ligatures() {
        assert_eq!(glyph_name_to_unicode("fi"), Some('ﬁ'));
        assert_eq!(glyph_name_to_unicode("fl"), Some('ﬂ'));
        assert_eq!(glyph_name_to_unicode("ffi"), Some('ﬃ'));
    }

    #[test]
    fn test_glyph_name_to_unicode_uni_xxxx() {
        assert_eq!(glyph_name_to_unicode("uni0041"), Some('A'));
        assert_eq!(glyph_name_to_unicode("uni2022"), Some('•'));
    }

    #[test]
    fn test_glyph_name_to_unicode_u_xxxx() {
        assert_eq!(glyph_name_to_unicode("u0041"), Some('A'));
        assert_eq!(glyph_name_to_unicode("u2022"), Some('•'));
    }

    #[test]
    fn test_glyph_name_to_unicode_unknown() {
        assert_eq!(glyph_name_to_unicode("unknownglyph"), None);
        assert_eq!(glyph_name_to_unicode(""), None);
    }

    #[test]
    fn test_char_to_unicode_custom_encoding() {
        let mut custom_map = HashMap::new();
        custom_map.insert(0x41, 'X');
        custom_map.insert(0x42, '•');

        let font = FontInfo {
            base_font: "CustomFont".to_string(),
            subtype: "Type1".to_string(),
            encoding: Encoding::Custom(custom_map),
            to_unicode: None,
            font_weight: None,
            flags: None,
            stem_v: None,
            ascent: 0.95,
            descent: -0.35,
            embedded_font_data: None,
            truetype_cmap: std::sync::OnceLock::new(),
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font: false,
            widths: None,
            first_char: None,
            last_char: None,
            font_matrix_a: 0.001,
            default_width: 1000.0,
            cid_to_gid_map: None,
            cid_system_info: None,
            cid_font_type: None,
            cid_widths: None,
            cid_default_width: 1000.0,
            has_explicit_dw: false,
            cff_gid_map: None,
            multi_char_map: HashMap::new(),
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names: std::collections::HashMap::new(),
            wmode: 0,
            cid_vertical_metrics: None,
            cid_default_vertical_metrics: VerticalMetrics::SPEC_DEFAULT,
            cjk_substitution: None,
        };

        assert_eq!(font.char_to_unicode(0x41), Some("X".to_string()));
        assert_eq!(font.char_to_unicode(0x42), Some("•".to_string()));
        assert_eq!(font.char_to_unicode(0x43), None);
    }

    /// Integration Test 1: ForceBold flag detection (PDF Spec Table 123, bit 19)
    #[test]
    fn test_get_font_weight_force_bold_flag() {
        let font_with_force_bold = FontInfo {
            base_font: "Helvetica".to_string(),
            subtype: "Type1".to_string(),
            encoding: Encoding::Standard("WinAnsiEncoding".to_string()),
            to_unicode: None,
            font_weight: None,
            flags: Some(0x80000),
            stem_v: None,
            ascent: 0.95,
            descent: -0.35,
            embedded_font_data: None,
            truetype_cmap: std::sync::OnceLock::new(),
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font: false,
            widths: None,
            first_char: None,
            last_char: None,
            font_matrix_a: 0.001,
            default_width: 1000.0,
            cid_to_gid_map: None,
            cid_system_info: None,
            cid_font_type: None,
            cid_widths: None,
            cid_default_width: 1000.0,
            has_explicit_dw: false,
            cff_gid_map: None,
            multi_char_map: HashMap::new(),
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names: std::collections::HashMap::new(),
            wmode: 0,
            cid_vertical_metrics: None,
            cid_default_vertical_metrics: VerticalMetrics::SPEC_DEFAULT,
            cjk_substitution: None,
        };

        assert_eq!(font_with_force_bold.get_font_weight(), FontWeight::Bold);
        assert!(font_with_force_bold.is_bold());

        let font_without_force_bold = FontInfo {
            base_font: "Helvetica".to_string(),
            subtype: "Type1".to_string(),
            encoding: Encoding::Standard("WinAnsiEncoding".to_string()),
            to_unicode: None,
            font_weight: None,
            flags: Some(0x40000),
            stem_v: None,
            ascent: 0.95,
            descent: -0.35,
            embedded_font_data: None,
            truetype_cmap: std::sync::OnceLock::new(),
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font: false,
            widths: None,
            first_char: None,
            last_char: None,
            font_matrix_a: 0.001,
            default_width: 1000.0,
            cid_to_gid_map: None,
            cid_system_info: None,
            cid_font_type: None,
            cid_widths: None,
            cid_default_width: 1000.0,
            has_explicit_dw: false,
            cff_gid_map: None,
            multi_char_map: HashMap::new(),
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names: std::collections::HashMap::new(),
            wmode: 0,
            cid_vertical_metrics: None,
            cid_default_vertical_metrics: VerticalMetrics::SPEC_DEFAULT,
            cjk_substitution: None,
        };

        assert_eq!(font_without_force_bold.get_font_weight(), FontWeight::Normal);
        assert!(!font_without_force_bold.is_bold());
    }

    /// Integration Test 2: StemV analysis for weight inference
    #[test]
    fn test_get_font_weight_stem_v_analysis() {
        let font_heavy_stem = FontInfo {
            base_font: "UnknownFont".to_string(),
            subtype: "Type1".to_string(),
            encoding: Encoding::Standard("WinAnsiEncoding".to_string()),
            to_unicode: None,
            font_weight: None,
            flags: None,
            stem_v: Some(120.0),
            ascent: 0.95,
            descent: -0.35,
            embedded_font_data: None,
            truetype_cmap: std::sync::OnceLock::new(),
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font: false,
            widths: None,
            first_char: None,
            last_char: None,
            font_matrix_a: 0.001,
            default_width: 1000.0,
            cid_to_gid_map: None,
            cid_system_info: None,
            cid_font_type: None,
            cid_widths: None,
            cid_default_width: 1000.0,
            has_explicit_dw: false,
            cff_gid_map: None,
            multi_char_map: HashMap::new(),
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names: std::collections::HashMap::new(),
            wmode: 0,
            cid_vertical_metrics: None,
            cid_default_vertical_metrics: VerticalMetrics::SPEC_DEFAULT,
            cjk_substitution: None,
        };

        assert_eq!(font_heavy_stem.get_font_weight(), FontWeight::Bold);
        assert!(font_heavy_stem.is_bold());

        let font_medium_stem = FontInfo {
            base_font: "UnknownFont".to_string(),
            subtype: "Type1".to_string(),
            encoding: Encoding::Standard("WinAnsiEncoding".to_string()),
            to_unicode: None,
            font_weight: None,
            flags: None,
            stem_v: Some(95.0),
            ascent: 0.95,
            descent: -0.35,
            embedded_font_data: None,
            truetype_cmap: std::sync::OnceLock::new(),
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font: false,
            widths: None,
            first_char: None,
            last_char: None,
            font_matrix_a: 0.001,
            default_width: 1000.0,
            cid_to_gid_map: None,
            cid_system_info: None,
            cid_font_type: None,
            cid_widths: None,
            cid_default_width: 1000.0,
            has_explicit_dw: false,
            cff_gid_map: None,
            multi_char_map: HashMap::new(),
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names: std::collections::HashMap::new(),
            wmode: 0,
            cid_vertical_metrics: None,
            cid_default_vertical_metrics: VerticalMetrics::SPEC_DEFAULT,
            cjk_substitution: None,
        };

        assert_eq!(font_medium_stem.get_font_weight(), FontWeight::Medium);
        assert!(!font_medium_stem.is_bold());

        let font_light_stem = FontInfo {
            base_font: "UnknownFont".to_string(),
            subtype: "Type1".to_string(),
            encoding: Encoding::Standard("WinAnsiEncoding".to_string()),
            to_unicode: None,
            font_weight: None,
            flags: None,
            stem_v: Some(70.0),
            ascent: 0.95,
            descent: -0.35,
            embedded_font_data: None,
            truetype_cmap: std::sync::OnceLock::new(),
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font: false,
            widths: None,
            first_char: None,
            last_char: None,
            font_matrix_a: 0.001,
            default_width: 1000.0,
            cid_to_gid_map: None,
            cid_system_info: None,
            cid_font_type: None,
            cid_widths: None,
            cid_default_width: 1000.0,
            has_explicit_dw: false,
            cff_gid_map: None,
            multi_char_map: HashMap::new(),
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names: std::collections::HashMap::new(),
            wmode: 0,
            cid_vertical_metrics: None,
            cid_default_vertical_metrics: VerticalMetrics::SPEC_DEFAULT,
            cjk_substitution: None,
        };

        assert_eq!(font_light_stem.get_font_weight(), FontWeight::Normal);
        assert!(!font_light_stem.is_bold());
    }

    /// Integration Test 3: Priority cascade (FontWeight > ForceBold > Name > StemV)
    #[test]
    fn test_get_font_weight_priority_cascade() {
        let font_explicit = FontInfo {
            base_font: "Helvetica-Bold".to_string(),
            subtype: "Type1".to_string(),
            encoding: Encoding::Standard("WinAnsiEncoding".to_string()),
            to_unicode: None,
            font_weight: Some(300),
            flags: Some(0x80000),
            stem_v: Some(120.0),
            ascent: 0.95,
            descent: -0.35,
            embedded_font_data: None,
            truetype_cmap: std::sync::OnceLock::new(),
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font: false,
            widths: None,
            first_char: None,
            last_char: None,
            font_matrix_a: 0.001,
            default_width: 1000.0,
            cid_to_gid_map: None,
            cid_system_info: None,
            cid_font_type: None,
            cid_widths: None,
            cid_default_width: 1000.0,
            has_explicit_dw: false,
            cff_gid_map: None,
            multi_char_map: HashMap::new(),
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names: std::collections::HashMap::new(),
            wmode: 0,
            cid_vertical_metrics: None,
            cid_default_vertical_metrics: VerticalMetrics::SPEC_DEFAULT,
            cjk_substitution: None,
        };

        assert_eq!(font_explicit.get_font_weight(), FontWeight::Light);
        assert!(!font_explicit.is_bold());

        let font_force_bold = FontInfo {
            base_font: "Helvetica".to_string(),
            subtype: "Type1".to_string(),
            encoding: Encoding::Standard("WinAnsiEncoding".to_string()),
            to_unicode: None,
            font_weight: None,
            flags: Some(0x80000),
            stem_v: Some(70.0),
            ascent: 0.95,
            descent: -0.35,
            embedded_font_data: None,
            truetype_cmap: std::sync::OnceLock::new(),
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font: false,
            widths: None,
            first_char: None,
            last_char: None,
            font_matrix_a: 0.001,
            default_width: 1000.0,
            cid_to_gid_map: None,
            cid_system_info: None,
            cid_font_type: None,
            cid_widths: None,
            cid_default_width: 1000.0,
            has_explicit_dw: false,
            cff_gid_map: None,
            multi_char_map: HashMap::new(),
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names: std::collections::HashMap::new(),
            wmode: 0,
            cid_vertical_metrics: None,
            cid_default_vertical_metrics: VerticalMetrics::SPEC_DEFAULT,
            cjk_substitution: None,
        };

        assert_eq!(font_force_bold.get_font_weight(), FontWeight::Bold);
        assert!(font_force_bold.is_bold());

        let font_name = FontInfo {
            base_font: "Helvetica-Bold".to_string(),
            subtype: "Type1".to_string(),
            encoding: Encoding::Standard("WinAnsiEncoding".to_string()),
            to_unicode: None,
            font_weight: None,
            flags: None,
            stem_v: Some(70.0),
            ascent: 0.95,
            descent: -0.35,
            embedded_font_data: None,
            truetype_cmap: std::sync::OnceLock::new(),
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font: false,
            widths: None,
            first_char: None,
            last_char: None,
            font_matrix_a: 0.001,
            default_width: 1000.0,
            cid_to_gid_map: None,
            cid_system_info: None,
            cid_font_type: None,
            cid_widths: None,
            cid_default_width: 1000.0,
            has_explicit_dw: false,
            cff_gid_map: None,
            multi_char_map: HashMap::new(),
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names: std::collections::HashMap::new(),
            wmode: 0,
            cid_vertical_metrics: None,
            cid_default_vertical_metrics: VerticalMetrics::SPEC_DEFAULT,
            cjk_substitution: None,
        };

        assert_eq!(font_name.get_font_weight(), FontWeight::Bold);
        assert!(font_name.is_bold());
    }

    /// Integration Test 4: Name heuristics for all weight categories
    #[test]
    fn test_get_font_weight_name_heuristics() {
        let font_black = FontInfo {
            base_font: "Helvetica-Black".to_string(),
            subtype: "Type1".to_string(),
            encoding: Encoding::Standard("WinAnsiEncoding".to_string()),
            to_unicode: None,
            font_weight: None,
            flags: None,
            stem_v: None,
            ascent: 0.95,
            descent: -0.35,
            embedded_font_data: None,
            truetype_cmap: std::sync::OnceLock::new(),
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font: false,
            widths: None,
            first_char: None,
            last_char: None,
            font_matrix_a: 0.001,
            default_width: 1000.0,
            cid_to_gid_map: None,
            cid_system_info: None,
            cid_font_type: None,
            cid_widths: None,
            cid_default_width: 1000.0,
            has_explicit_dw: false,
            cff_gid_map: None,
            multi_char_map: HashMap::new(),
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names: std::collections::HashMap::new(),
            wmode: 0,
            cid_vertical_metrics: None,
            cid_default_vertical_metrics: VerticalMetrics::SPEC_DEFAULT,
            cjk_substitution: None,
        };
        assert_eq!(font_black.get_font_weight(), FontWeight::Black);
        assert!(font_black.is_bold());

        let font_extrabold = FontInfo {
            base_font: "Arial-ExtraBold".to_string(),
            subtype: "Type1".to_string(),
            encoding: Encoding::Standard("WinAnsiEncoding".to_string()),
            to_unicode: None,
            font_weight: None,
            flags: None,
            stem_v: None,
            ascent: 0.95,
            descent: -0.35,
            embedded_font_data: None,
            truetype_cmap: std::sync::OnceLock::new(),
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font: false,
            widths: None,
            first_char: None,
            last_char: None,
            font_matrix_a: 0.001,
            default_width: 1000.0,
            cid_to_gid_map: None,
            cid_system_info: None,
            cid_font_type: None,
            cid_widths: None,
            cid_default_width: 1000.0,
            has_explicit_dw: false,
            cff_gid_map: None,
            multi_char_map: HashMap::new(),
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names: std::collections::HashMap::new(),
            wmode: 0,
            cid_vertical_metrics: None,
            cid_default_vertical_metrics: VerticalMetrics::SPEC_DEFAULT,
            cjk_substitution: None,
        };
        assert_eq!(font_extrabold.get_font_weight(), FontWeight::ExtraBold);
        assert!(font_extrabold.is_bold());

        let font_bold = FontInfo {
            base_font: "TimesNewRoman-Bold".to_string(),
            subtype: "Type1".to_string(),
            encoding: Encoding::Standard("WinAnsiEncoding".to_string()),
            to_unicode: None,
            font_weight: None,
            flags: None,
            stem_v: None,
            ascent: 0.95,
            descent: -0.35,
            embedded_font_data: None,
            truetype_cmap: std::sync::OnceLock::new(),
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font: false,
            widths: None,
            first_char: None,
            last_char: None,
            font_matrix_a: 0.001,
            default_width: 1000.0,
            cid_to_gid_map: None,
            cid_system_info: None,
            cid_font_type: None,
            cid_widths: None,
            cid_default_width: 1000.0,
            has_explicit_dw: false,
            cff_gid_map: None,
            multi_char_map: HashMap::new(),
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names: std::collections::HashMap::new(),
            wmode: 0,
            cid_vertical_metrics: None,
            cid_default_vertical_metrics: VerticalMetrics::SPEC_DEFAULT,
            cjk_substitution: None,
        };
        assert_eq!(font_bold.get_font_weight(), FontWeight::Bold);
        assert!(font_bold.is_bold());

        let font_semibold = FontInfo {
            base_font: "Arial-SemiBold".to_string(),
            subtype: "Type1".to_string(),
            encoding: Encoding::Standard("WinAnsiEncoding".to_string()),
            to_unicode: None,
            font_weight: None,
            flags: None,
            stem_v: None,
            ascent: 0.95,
            descent: -0.35,
            embedded_font_data: None,
            truetype_cmap: std::sync::OnceLock::new(),
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font: false,
            widths: None,
            first_char: None,
            last_char: None,
            font_matrix_a: 0.001,
            default_width: 1000.0,
            cid_to_gid_map: None,
            cid_system_info: None,
            cid_font_type: None,
            cid_widths: None,
            cid_default_width: 1000.0,
            has_explicit_dw: false,
            cff_gid_map: None,
            multi_char_map: HashMap::new(),
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names: std::collections::HashMap::new(),
            wmode: 0,
            cid_vertical_metrics: None,
            cid_default_vertical_metrics: VerticalMetrics::SPEC_DEFAULT,
            cjk_substitution: None,
        };
        assert_eq!(font_semibold.get_font_weight(), FontWeight::SemiBold);
        assert!(font_semibold.is_bold());

        let font_medium = FontInfo {
            base_font: "Roboto-Medium".to_string(),
            subtype: "Type1".to_string(),
            encoding: Encoding::Standard("WinAnsiEncoding".to_string()),
            to_unicode: None,
            font_weight: None,
            flags: None,
            stem_v: None,
            ascent: 0.95,
            descent: -0.35,
            embedded_font_data: None,
            truetype_cmap: std::sync::OnceLock::new(),
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font: false,
            widths: None,
            first_char: None,
            last_char: None,
            font_matrix_a: 0.001,
            default_width: 1000.0,
            cid_to_gid_map: None,
            cid_system_info: None,
            cid_font_type: None,
            cid_widths: None,
            cid_default_width: 1000.0,
            has_explicit_dw: false,
            cff_gid_map: None,
            multi_char_map: HashMap::new(),
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names: std::collections::HashMap::new(),
            wmode: 0,
            cid_vertical_metrics: None,
            cid_default_vertical_metrics: VerticalMetrics::SPEC_DEFAULT,
            cjk_substitution: None,
        };
        assert_eq!(font_medium.get_font_weight(), FontWeight::Medium);
        assert!(!font_medium.is_bold());

        let font_light = FontInfo {
            base_font: "Helvetica-Light".to_string(),
            subtype: "Type1".to_string(),
            encoding: Encoding::Standard("WinAnsiEncoding".to_string()),
            to_unicode: None,
            font_weight: None,
            flags: None,
            stem_v: None,
            ascent: 0.95,
            descent: -0.35,
            embedded_font_data: None,
            truetype_cmap: std::sync::OnceLock::new(),
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font: false,
            widths: None,
            first_char: None,
            last_char: None,
            font_matrix_a: 0.001,
            default_width: 1000.0,
            cid_to_gid_map: None,
            cid_system_info: None,
            cid_font_type: None,
            cid_widths: None,
            cid_default_width: 1000.0,
            has_explicit_dw: false,
            cff_gid_map: None,
            multi_char_map: HashMap::new(),
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names: std::collections::HashMap::new(),
            wmode: 0,
            cid_vertical_metrics: None,
            cid_default_vertical_metrics: VerticalMetrics::SPEC_DEFAULT,
            cjk_substitution: None,
        };
        assert_eq!(font_light.get_font_weight(), FontWeight::Light);
        assert!(!font_light.is_bold());

        let font_extralight = FontInfo {
            base_font: "Roboto-ExtraLight".to_string(),
            subtype: "Type1".to_string(),
            encoding: Encoding::Standard("WinAnsiEncoding".to_string()),
            to_unicode: None,
            font_weight: None,
            flags: None,
            stem_v: None,
            ascent: 0.95,
            descent: -0.35,
            embedded_font_data: None,
            truetype_cmap: std::sync::OnceLock::new(),
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font: false,
            widths: None,
            first_char: None,
            last_char: None,
            font_matrix_a: 0.001,
            default_width: 1000.0,
            cid_to_gid_map: None,
            cid_system_info: None,
            cid_font_type: None,
            cid_widths: None,
            cid_default_width: 1000.0,
            has_explicit_dw: false,
            cff_gid_map: None,
            multi_char_map: HashMap::new(),
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names: std::collections::HashMap::new(),
            wmode: 0,
            cid_vertical_metrics: None,
            cid_default_vertical_metrics: VerticalMetrics::SPEC_DEFAULT,
            cjk_substitution: None,
        };
        assert_eq!(font_extralight.get_font_weight(), FontWeight::ExtraLight);
        assert!(!font_extralight.is_bold());

        let font_thin = FontInfo {
            base_font: "HelveticaNeue-Thin".to_string(),
            subtype: "Type1".to_string(),
            encoding: Encoding::Standard("WinAnsiEncoding".to_string()),
            to_unicode: None,
            font_weight: None,
            flags: None,
            stem_v: None,
            ascent: 0.95,
            descent: -0.35,
            embedded_font_data: None,
            truetype_cmap: std::sync::OnceLock::new(),
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font: false,
            widths: None,
            first_char: None,
            last_char: None,
            font_matrix_a: 0.001,
            default_width: 1000.0,
            cid_to_gid_map: None,
            cid_system_info: None,
            cid_font_type: None,
            cid_widths: None,
            cid_default_width: 1000.0,
            has_explicit_dw: false,
            cff_gid_map: None,
            multi_char_map: HashMap::new(),
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names: std::collections::HashMap::new(),
            wmode: 0,
            cid_vertical_metrics: None,
            cid_default_vertical_metrics: VerticalMetrics::SPEC_DEFAULT,
            cjk_substitution: None,
        };
        assert_eq!(font_thin.get_font_weight(), FontWeight::Thin);
        assert!(!font_thin.is_bold());

        let font_normal = FontInfo {
            base_font: "Helvetica".to_string(),
            subtype: "Type1".to_string(),
            encoding: Encoding::Standard("WinAnsiEncoding".to_string()),
            to_unicode: None,
            font_weight: None,
            flags: None,
            stem_v: None,
            ascent: 0.95,
            descent: -0.35,
            embedded_font_data: None,
            truetype_cmap: std::sync::OnceLock::new(),
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font: false,
            widths: None,
            first_char: None,
            last_char: None,
            font_matrix_a: 0.001,
            default_width: 1000.0,
            cid_to_gid_map: None,
            cid_system_info: None,
            cid_font_type: None,
            cid_widths: None,
            cid_default_width: 1000.0,
            has_explicit_dw: false,
            cff_gid_map: None,
            multi_char_map: HashMap::new(),
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names: std::collections::HashMap::new(),
            wmode: 0,
            cid_vertical_metrics: None,
            cid_default_vertical_metrics: VerticalMetrics::SPEC_DEFAULT,
            cjk_substitution: None,
        };
        assert_eq!(font_normal.get_font_weight(), FontWeight::Normal);
        assert!(!font_normal.is_bold());
    }

    /// Test CIDToGIDMap Identity mapping
    /// Per PDF Spec ISO 32000-1:2008, Section 9.7.4.2
    #[test]
    fn test_cid_to_gid_identity() {
        let identity_map = CIDToGIDMap::Identity;

        assert_eq!(identity_map.get_gid(0), 0);
        assert_eq!(identity_map.get_gid(100), 100);
        assert_eq!(identity_map.get_gid(0xFFFF), 0xFFFF);
    }

    /// Test CIDToGIDMap Explicit mapping
    /// Verifies that explicit GID arrays are looked up correctly
    #[test]
    fn test_cid_to_gid_explicit() {
        let gid_array = vec![10, 20, 30];
        let explicit_map = CIDToGIDMap::Explicit(gid_array);

        assert_eq!(explicit_map.get_gid(0), 10);
        assert_eq!(explicit_map.get_gid(1), 20);
        assert_eq!(explicit_map.get_gid(2), 30);

        assert_eq!(explicit_map.get_gid(3), 3);
        assert_eq!(explicit_map.get_gid(100), 100);
    }

    #[test]
    fn test_gid_to_glyph_name_ascii_range() {
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0x20), Some("space"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0x41), Some("A"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0x61), Some("a"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0x30), Some("zero"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0x7E), Some("asciitilde"));
    }

    #[test]
    fn test_gid_to_glyph_name_windows1252_symbols() {
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0x80), Some("euro"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0x83), Some("florin"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0x85), Some("ellipsis"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0x8C), Some("OE"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0x9C), Some("oe"));

        assert_eq!(FontInfo::gid_to_standard_glyph_name(0x8A), Some("Scaron"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0x9A), Some("scaron"));

        assert_eq!(FontInfo::gid_to_standard_glyph_name(0x91), Some("quoteleft"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0x92), Some("quoteright"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0x93), Some("quotedblleft"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0x94), Some("quotedblright"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0x96), Some("endash"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0x97), Some("emdash"));
    }

    #[test]
    fn test_gid_to_glyph_name_latin1_supplement() {
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xA2), Some("cent"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xA3), Some("sterling"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xA4), Some("currency"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xA5), Some("yen"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xA9), Some("copyright"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xAE), Some("registered"));

        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xB0), Some("degree"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xB1), Some("plusminus"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xD7), Some("multiply"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xF7), Some("divide"));
    }

    #[test]
    fn test_gid_to_glyph_name_uppercase_accented() {
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xC0), Some("Agrave"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xC1), Some("Aacute"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xC2), Some("Acircumflex"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xC3), Some("Atilde"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xC4), Some("Adieresis"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xC5), Some("Aring"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xC6), Some("AE"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xC7), Some("Ccedilla"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xD1), Some("Ntilde"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xD6), Some("Odieresis"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xDC), Some("Udieresis"));
    }

    #[test]
    fn test_gid_to_glyph_name_lowercase_accented() {
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xE0), Some("agrave"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xE1), Some("aacute"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xE2), Some("acircumflex"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xE3), Some("atilde"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xE4), Some("adieresis"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xE5), Some("aring"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xE6), Some("ae"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xE7), Some("ccedilla"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xF1), Some("ntilde"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xF6), Some("odieresis"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xFC), Some("udieresis"));
    }

    #[test]
    fn test_gid_to_glyph_name_special_characters() {
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xAA), Some("ordfeminine"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xBA), Some("ordmasculine"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xB2), Some("twosuperior"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xB3), Some("threesuperior"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xB9), Some("onesuperior"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xBC), Some("onequarter"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xBD), Some("onehalf"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xBE), Some("threequarters"));
    }

    #[test]
    fn test_gid_to_glyph_name_undefined_codes() {
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0x81), None);
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0x8D), None);
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0x8F), None);
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0x90), None);
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0x9D), None);
    }

    #[test]
    fn test_gid_to_glyph_name_out_of_range() {
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0x100), None);
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0x1000), None);
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xFFFF), None);
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0x0000), None);
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0x0001), None);
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0x001F), None);
    }

    #[test]
    fn test_agl_fallback_euro_sign() {
        // Test that CID 0x80 (Euro sign) maps through AGL correctly
        // This is a real-world case: Type0 fonts without ToUnicode often need Euro mapping
        // ~keep
        let glyph_name = FontInfo::gid_to_standard_glyph_name(0x80).expect("0x80 should map to euro");
        assert_eq!(glyph_name, "euro");

        assert!(ADOBE_GLYPH_LIST.get(glyph_name).is_some());

        if let Some(&unicode_char) = ADOBE_GLYPH_LIST.get(glyph_name) {
            assert_eq!(unicode_char as u32, 0x20AC);
        }
    }

    #[test]
    fn test_agl_fallback_extended_latin_coverage() {
        let test_cases = vec![
            (0x80, "euro", 0x20AC),
            (0x82, "quotesinglbase", 0x201A),
            (0x83, "florin", 0x0192),
            (0x84, "quotedblbase", 0x201E),
            (0x85, "ellipsis", 0x2026),
            (0xA9, "copyright", 0x00A9), // Copyright
            (0xAE, "registered", 0x00AE),
            (0xB0, "degree", 0x00B0),
            (0xC1, "Aacute", 0x00C1),
            (0xE1, "aacute", 0x00E1),
        ];

        for (gid, expected_glyph, expected_unicode) in test_cases {
            let glyph_name = FontInfo::gid_to_standard_glyph_name(gid as u16)
                .unwrap_or_else(|| panic!("GID 0x{:02X} should map to a glyph name", gid));
            assert_eq!(glyph_name, expected_glyph);

            if let Some(&unicode_char) = ADOBE_GLYPH_LIST.get(glyph_name) {
                assert_eq!(unicode_char as u32, expected_unicode);
            } else {
                panic!("Glyph '{}' should exist in Adobe Glyph List", glyph_name);
            }
        }
    }

    #[test]
    fn test_agl_fallback_priority_integration() {
        // Integration test: Verify AGL fallback would activate for unmapped Type0 CIDs
        // This simulates the Priority 5 fallback in char_to_unicode()
        //
        // Scenario:
        // 1. Type0 font with Identity-H CMap
        // 2. No ToUnicode CMap
        // 3. No TrueType cmap
        // 4. CID 0xC1 (Á - A with acute accent) - common in Spanish/French documents
        //
        // Expected: CID 0xC1 -> GID 0xC1 -> "Aacute" -> U+00C1 ~keep

        let glyph_name = FontInfo::gid_to_standard_glyph_name(0xC1).expect("GID 0xC1 should map to Aacute");
        assert_eq!(glyph_name, "Aacute");

        assert!(ADOBE_GLYPH_LIST.get("Aacute").is_some());

        if let Some(&unicode_char) = ADOBE_GLYPH_LIST.get("Aacute") {
            let result = unicode_char.to_string();
            assert_eq!(unicode_char as u32, 0x00C1);
            assert!(!result.is_empty());
        }
    }

    #[test]
    fn test_get_glyph_width_uses_cid_widths() {
        let mut cid_widths = HashMap::new();
        cid_widths.insert(1u16, 500.0f32);
        cid_widths.insert(2u16, 600.0f32);
        cid_widths.insert(3u16, 700.0f32);

        let font = FontInfo {
            base_font: "CIDFont".to_string(),
            subtype: "Type0".to_string(),
            encoding: Encoding::Identity,
            to_unicode: None,
            font_weight: None,
            flags: None,
            stem_v: None,
            ascent: 0.95,
            descent: -0.35,
            embedded_font_data: None,
            truetype_cmap: std::sync::OnceLock::new(),
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font: false,
            widths: None,
            first_char: None,
            last_char: None,
            font_matrix_a: 0.001,
            default_width: 1000.0,
            cid_to_gid_map: None,
            cid_system_info: None,
            cid_font_type: None,
            cid_widths: Some(cid_widths),
            cid_default_width: 1000.0,
            has_explicit_dw: false,
            cff_gid_map: None,
            multi_char_map: HashMap::new(),
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names: std::collections::HashMap::new(),
            wmode: 0,
            cid_vertical_metrics: None,
            cid_default_vertical_metrics: VerticalMetrics::SPEC_DEFAULT,
            cjk_substitution: None,
        };

        assert_eq!(font.get_glyph_width(1), 500.0);
        assert_eq!(font.get_glyph_width(2), 600.0);
        assert_eq!(font.get_glyph_width(3), 700.0);

        assert_eq!(font.get_glyph_width(100), 1000.0);
    }

    #[test]
    fn test_get_glyph_width_cid_default_width() {
        let mut cid_widths = HashMap::new();
        cid_widths.insert(1u16, 500.0f32);

        let font = FontInfo {
            base_font: "CIDFont".to_string(),
            subtype: "Type0".to_string(),
            encoding: Encoding::Identity,
            to_unicode: None,
            font_weight: None,
            flags: None,
            stem_v: None,
            ascent: 0.95,
            descent: -0.35,
            embedded_font_data: None,
            truetype_cmap: std::sync::OnceLock::new(),
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font: false,
            widths: None,
            first_char: None,
            last_char: None,
            font_matrix_a: 0.001,
            default_width: 500.0,
            cid_to_gid_map: None,
            cid_system_info: None,
            cid_font_type: None,
            cid_widths: Some(cid_widths),
            cid_default_width: 800.0,
            has_explicit_dw: true,
            cff_gid_map: None,
            multi_char_map: HashMap::new(),
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names: std::collections::HashMap::new(),
            wmode: 0,
            cid_vertical_metrics: None,
            cid_default_vertical_metrics: VerticalMetrics::SPEC_DEFAULT,
            cjk_substitution: None,
        };

        assert_eq!(font.get_glyph_width(1), 500.0);

        // Other CIDs use cid_default_width (not default_width) when has_explicit_dw=true
        // ~keep
        assert_eq!(font.get_glyph_width(2), 800.0);
        assert_eq!(font.get_glyph_width(999), 800.0);
    }

    #[test]
    fn test_get_glyph_width_no_cid_widths_uses_default() {
        let font = FontInfo {
            base_font: "SimpleFont".to_string(),
            subtype: "Type1".to_string(),
            encoding: Encoding::Standard("WinAnsiEncoding".to_string()),
            to_unicode: None,
            font_weight: None,
            flags: None,
            stem_v: None,
            ascent: 0.95,
            descent: -0.35,
            embedded_font_data: None,
            truetype_cmap: std::sync::OnceLock::new(),
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font: false,
            widths: None,
            first_char: None,
            last_char: None,
            font_matrix_a: 0.001,
            default_width: 600.0,
            cid_to_gid_map: None,
            cid_system_info: None,
            cid_font_type: None,
            cid_widths: None,
            cid_default_width: 1000.0,
            has_explicit_dw: false,
            cff_gid_map: None,
            multi_char_map: HashMap::new(),
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names: std::collections::HashMap::new(),
            wmode: 0,
            cid_vertical_metrics: None,
            cid_default_vertical_metrics: VerticalMetrics::SPEC_DEFAULT,
            cjk_substitution: None,
        };

        assert_eq!(font.get_glyph_width(1), 600.0);
        assert_eq!(font.get_glyph_width(65), 600.0);
    }

    #[test]
    fn test_cid_widths_large_range() {
        let mut cid_widths = HashMap::new();
        for cid in 1u16..=100 {
            cid_widths.insert(cid, 1000.0f32);
        }
        cid_widths.insert(200, 500.0);
        cid_widths.insert(201, 600.0);

        let font = FontInfo {
            base_font: "CJKFont".to_string(),
            subtype: "Type0".to_string(),
            encoding: Encoding::Identity,
            to_unicode: None,
            font_weight: None,
            flags: None,
            stem_v: None,
            ascent: 0.95,
            descent: -0.35,
            embedded_font_data: None,
            truetype_cmap: std::sync::OnceLock::new(),
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font: false,
            widths: None,
            first_char: None,
            last_char: None,
            font_matrix_a: 0.001,
            default_width: 500.0,
            cid_to_gid_map: None,
            cid_system_info: Some(CIDSystemInfo {
                registry: "Adobe".to_string(),
                ordering: "Japan1".to_string(),
                supplement: 4,
            }),
            cid_font_type: Some("CIDFontType2".to_string()),
            cid_widths: Some(cid_widths),
            cid_default_width: 1000.0,
            has_explicit_dw: false,
            cff_gid_map: None,
            multi_char_map: HashMap::new(),
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names: std::collections::HashMap::new(),
            wmode: 0,
            cid_vertical_metrics: None,
            cid_default_vertical_metrics: VerticalMetrics::SPEC_DEFAULT,
            cjk_substitution: None,
        };

        assert_eq!(font.get_glyph_width(1), 1000.0);
        assert_eq!(font.get_glyph_width(50), 1000.0);
        assert_eq!(font.get_glyph_width(100), 1000.0);

        assert_eq!(font.get_glyph_width(200), 500.0);
        assert_eq!(font.get_glyph_width(201), 600.0);

        // F15 fix: has_explicit_dw=false → fall back to default_width (500.0), not cid_default_width.
        // When /DW is not explicit in the PDF, we cannot trust cid_default_width as authoritative.
        // ~keep
        assert_eq!(font.get_glyph_width(300), 500.0);
    }

    fn make_font(overrides: impl FnOnce(&mut FontInfo)) -> FontInfo {
        let mut f = FontInfo {
            base_font: "TestFont".to_string(),
            subtype: "Type1".to_string(),
            encoding: Encoding::Standard("WinAnsiEncoding".to_string()),
            to_unicode: None,
            font_weight: None,
            flags: None,
            stem_v: None,
            ascent: 0.95,
            descent: -0.35,
            embedded_font_data: None,
            truetype_cmap: std::sync::OnceLock::new(),
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font: false,
            widths: None,
            first_char: None,
            last_char: None,
            font_matrix_a: 0.001,
            default_width: 500.0,
            cid_to_gid_map: None,
            cid_system_info: None,
            cid_font_type: None,
            cid_widths: None,
            cid_default_width: 1000.0,
            has_explicit_dw: false,
            cff_gid_map: None,
            multi_char_map: HashMap::new(),
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names: std::collections::HashMap::new(),
            wmode: 0,
            cid_vertical_metrics: None,
            cid_default_vertical_metrics: VerticalMetrics::SPEC_DEFAULT,
            cjk_substitution: None,
        };
        overrides(&mut f);
        f
    }

    // The critical case: a Type0 / Identity-ordered subset with no ToUnicode
    // and no embedded cmap severs every path to Unicode → Fallback. ~keep
    #[test]
    fn best_mapping_provenance_fallback_on_severed_identity_type0() {
        let f = make_font(|f| {
            f.subtype = "Type0".to_string();
            f.encoding = Encoding::Standard("Identity-H".to_string());
            f.cid_system_info = Some(CIDSystemInfo {
                registry: "Adobe".to_string(),
                ordering: "Identity".to_string(),
                supplement: 0,
            });
            f.to_unicode = None;
        });
        assert_eq!(f.best_mapping_provenance(), crate::fonts::MappingProvenance::Fallback);
    }

    #[test]
    fn best_mapping_provenance_fallback_type0_without_cidsysteminfo() {
        let f = make_font(|f| {
            f.subtype = "Type0".to_string();
            f.encoding = Encoding::Standard("Identity-H".to_string());
            f.cid_system_info = None;
            f.to_unicode = None;
        });
        assert_eq!(f.best_mapping_provenance(), crate::fonts::MappingProvenance::Fallback);
    }

    #[test]
    fn best_mapping_provenance_predefined_for_known_collection() {
        let f = make_font(|f| {
            f.subtype = "Type0".to_string();
            f.cid_system_info = Some(CIDSystemInfo {
                registry: "Adobe".to_string(),
                ordering: "Japan1".to_string(),
                supplement: 6,
            });
        });
        assert_eq!(
            f.best_mapping_provenance(),
            crate::fonts::MappingProvenance::PredefinedCMap
        );
    }

    #[test]
    fn best_mapping_provenance_encoding_for_simple_font() {
        let f = make_font(|_| {});
        assert_eq!(
            f.best_mapping_provenance(),
            crate::fonts::MappingProvenance::EncodingName
        );
    }

    // =========================================================================
    // get_space_glyph_width — the space advance drives the geometric word-gap
    // threshold, so it must be a REAL space advance, never an arbitrary glyph.
    // Regression guard for justified TJ words glued together. ~keep
    // =========================================================================

    #[test]
    fn space_width_identity_type0_ignores_cid32_glyph() {
        // Under Identity-H, character code 0x20 maps to CID 32 — an
        // arbitrary font glyph (real repro: TimesNewRomanPSMT reports 563 units
        // ≈ 0.56 em), NOT the space. Trusting it as the space advance inflated
        // the word-gap threshold (0.75 × 0.56 em) so far that real ~0.25 em
        // justified word gaps were suppressed and adjacent words glued together
        // ("All rights reserved" -> "Allrightsreserved"). The reference must
        // fall back to the 0.25 em (250-unit) typographic default instead. ~keep
        let mut cid_widths = HashMap::new();
        cid_widths.insert(0x20_u16, 563.0);
        let font = make_font(|f| {
            f.subtype = "Type0".to_string();
            f.encoding = Encoding::Identity;
            f.cid_widths = Some(cid_widths);
        });
        assert_eq!(
            font.get_space_glyph_width(),
            250.0,
            "Identity Type0 must not treat the CID-32 glyph width as the space advance"
        );
    }

    #[test]
    fn space_width_identity_type0_without_cid32_defaults() {
        let font = make_font(|f| {
            f.subtype = "Type0".to_string();
            f.encoding = Encoding::Identity;
            f.cid_widths = Some(HashMap::new());
        });
        assert_eq!(font.get_space_glyph_width(), 250.0);
    }

    #[test]
    fn space_width_non_identity_type0_trusts_explicit_space_cid() {
        // A non-Identity predefined CMap can genuinely place the space at code
        // 0x20, so an explicit /W entry there is a real space advance and is
        // kept — only Identity encoding remaps 0x20 to an arbitrary CID. ~keep
        let mut cid_widths = HashMap::new();
        cid_widths.insert(0x20_u16, 280.0);
        let font = make_font(|f| {
            f.subtype = "Type0".to_string();
            f.encoding = Encoding::Standard("Predefined-CMap".to_string());
            f.cid_widths = Some(cid_widths);
        });
        assert_eq!(font.get_space_glyph_width(), 280.0);
    }

    #[test]
    fn space_width_simple_font_uses_explicit_widths_space() {
        let font = make_font(|f| {
            f.subtype = "Type1".to_string();
            f.first_char = Some(32);
            f.widths = Some(vec![260.0, 500.0, 500.0]);
        });
        assert_eq!(font.get_space_glyph_width(), 260.0);
    }

    #[test]
    fn test_parse_cid_widths_array_format() {
        let mut dict: HashMap<String, Object> = HashMap::new();
        dict.insert(
            "W".to_string(),
            Object::Array(vec![
                Object::Integer(10),
                Object::Array(vec![Object::Integer(500), Object::Integer(600), Object::Integer(700)]),
            ]),
        );
        let widths = FontInfo::parse_cid_widths(&dict, "Test").unwrap();
        assert_eq!(widths.get(&10), Some(&500.0));
        assert_eq!(widths.get(&11), Some(&600.0));
        assert_eq!(widths.get(&12), Some(&700.0));
        assert_eq!(widths.get(&13), None);
    }

    #[test]
    fn test_parse_cid_widths_range_format() {
        let mut dict: HashMap<String, Object> = HashMap::new();
        dict.insert(
            "W".to_string(),
            Object::Array(vec![Object::Integer(100), Object::Integer(105), Object::Integer(300)]),
        );
        let widths = FontInfo::parse_cid_widths(&dict, "Test").unwrap();
        for cid in 100..=105 {
            assert_eq!(widths.get(&cid), Some(&300.0), "CID {} should be 300", cid);
        }
        assert_eq!(widths.get(&106), None);
    }

    #[test]
    fn test_parse_cid_widths_mixed_formats() {
        let mut dict: HashMap<String, Object> = HashMap::new();
        dict.insert(
            "W".to_string(),
            Object::Array(vec![
                Object::Integer(1),
                Object::Array(vec![Object::Integer(200), Object::Integer(300)]),
                Object::Integer(50),
                Object::Integer(52),
                Object::Integer(400),
            ]),
        );
        let widths = FontInfo::parse_cid_widths(&dict, "Test").unwrap();
        assert_eq!(widths.get(&1), Some(&200.0));
        assert_eq!(widths.get(&2), Some(&300.0));
        assert_eq!(widths.get(&50), Some(&400.0));
        assert_eq!(widths.get(&51), Some(&400.0));
        assert_eq!(widths.get(&52), Some(&400.0));
    }

    #[test]
    fn test_parse_cid_widths_real_values() {
        let mut dict: HashMap<String, Object> = HashMap::new();
        dict.insert(
            "W".to_string(),
            Object::Array(vec![Object::Integer(5), Object::Array(vec![Object::Real(123.5)])]),
        );
        let widths = FontInfo::parse_cid_widths(&dict, "Test").unwrap();
        assert_eq!(widths.get(&5), Some(&123.5));
    }

    #[test]
    fn test_parse_cid_widths_empty_array() {
        let mut dict: HashMap<String, Object> = HashMap::new();
        dict.insert("W".to_string(), Object::Array(vec![]));
        assert!(FontInfo::parse_cid_widths(&dict, "Test").is_none());
    }

    #[test]
    fn test_parse_cid_widths_missing_w() {
        let dict: HashMap<String, Object> = HashMap::new();
        assert!(FontInfo::parse_cid_widths(&dict, "Test").is_none());
    }

    #[test]
    fn test_parse_cid_widths_non_integer_start() {
        let mut dict: HashMap<String, Object> = HashMap::new();
        dict.insert(
            "W".to_string(),
            Object::Array(vec![
                Object::Name("bad".to_string()),
                Object::Integer(10),
                Object::Array(vec![Object::Integer(500)]),
            ]),
        );
        let widths = FontInfo::parse_cid_widths(&dict, "Test").unwrap();
        assert_eq!(widths.get(&10), Some(&500.0));
    }

    #[test]
    fn test_parse_cid_widths_truncated_range() {
        let mut dict: HashMap<String, Object> = HashMap::new();
        dict.insert(
            "W".to_string(),
            Object::Array(vec![Object::Integer(10), Object::Integer(15)]),
        );
        assert!(FontInfo::parse_cid_widths(&dict, "Test").is_none());
    }

    #[test]
    fn test_parse_cid_widths_unexpected_second_element() {
        let mut dict: HashMap<String, Object> = HashMap::new();
        dict.insert(
            "W".to_string(),
            Object::Array(vec![Object::Integer(10), Object::Name("bad".to_string())]),
        );
        assert!(FontInfo::parse_cid_widths(&dict, "Test").is_none());
    }

    #[test]
    fn test_parse_cid_widths_range_with_bad_width() {
        let mut dict: HashMap<String, Object> = HashMap::new();
        dict.insert(
            "W".to_string(),
            Object::Array(vec![
                Object::Integer(1),
                Object::Integer(3),
                Object::Name("notanumber".to_string()),
            ]),
        );
        assert!(FontInfo::parse_cid_widths(&dict, "Test").is_none());
    }

    #[test]
    fn test_parse_cid_widths_range_with_real_width() {
        let mut dict: HashMap<String, Object> = HashMap::new();
        dict.insert(
            "W".to_string(),
            Object::Array(vec![Object::Integer(10), Object::Integer(12), Object::Real(750.5)]),
        );
        let widths = FontInfo::parse_cid_widths(&dict, "Test").unwrap();
        assert_eq!(widths.get(&10), Some(&750.5));
        assert_eq!(widths.get(&11), Some(&750.5));
        assert_eq!(widths.get(&12), Some(&750.5));
    }

    /// `/W2` Form A: `c [ w1y v_x v_y w1y v_x v_y … ]` assigns successive
    /// triples to CIDs `c`, `c+1`, `c+2`, … Drives per-CID lookups for
    /// vertical advance and vertical-origin offset on tategaki layouts.
    #[test]
    fn test_parse_w2_explicit_array_form() {
        let mut dict: HashMap<String, Object> = HashMap::new();
        dict.insert(
            "W2".to_string(),
            Object::Array(vec![
                Object::Integer(10),
                Object::Array(vec![
                    Object::Integer(-880),
                    Object::Integer(500),
                    Object::Integer(900),
                    Object::Integer(-1000),
                    Object::Integer(520),
                    Object::Integer(850),
                ]),
            ]),
        );
        let metrics = FontInfo::parse_cid_vertical_metrics(&dict, "Test").unwrap();
        assert_eq!(
            metrics.get(&10),
            Some(&VerticalMetrics {
                w1y: -880.0,
                v_x: 500.0,
                v_y: 900.0
            })
        );
        assert_eq!(
            metrics.get(&11),
            Some(&VerticalMetrics {
                w1y: -1000.0,
                v_x: 520.0,
                v_y: 850.0
            })
        );
        assert_eq!(metrics.get(&12), None);
    }

    /// `/W2` Form B: `c_first c_last w1y v_x v_y` assigns the same metrics
    /// to every CID in the inclusive range.
    #[test]
    fn test_parse_w2_range_form() {
        let mut dict: HashMap<String, Object> = HashMap::new();
        dict.insert(
            "W2".to_string(),
            Object::Array(vec![
                Object::Integer(100),
                Object::Integer(102),
                Object::Integer(-1000),
                Object::Integer(500),
                Object::Integer(880),
            ]),
        );
        let metrics = FontInfo::parse_cid_vertical_metrics(&dict, "Test").unwrap();
        let expected = VerticalMetrics {
            w1y: -1000.0,
            v_x: 500.0,
            v_y: 880.0,
        };
        assert_eq!(metrics.get(&100), Some(&expected));
        assert_eq!(metrics.get(&101), Some(&expected));
        assert_eq!(metrics.get(&102), Some(&expected));
        assert_eq!(metrics.get(&103), None);
        assert_eq!(metrics.get(&99), None);
    }

    /// `/W2` Form A and Form B can be intermixed in a single array. Real
    /// CIDFonts use this routinely — explicit triples for outliers and
    /// ranges for runs of full-width CJK glyphs.
    #[test]
    fn test_parse_w2_mixed_forms() {
        let mut dict: HashMap<String, Object> = HashMap::new();
        dict.insert(
            "W2".to_string(),
            Object::Array(vec![
                Object::Integer(5),
                Object::Array(vec![Object::Integer(-900), Object::Integer(490), Object::Integer(870)]),
                Object::Integer(200),
                Object::Integer(201),
                Object::Integer(-1000),
                Object::Integer(500),
                Object::Integer(880),
            ]),
        );
        let metrics = FontInfo::parse_cid_vertical_metrics(&dict, "Test").unwrap();
        assert_eq!(
            metrics.get(&5),
            Some(&VerticalMetrics {
                w1y: -900.0,
                v_x: 490.0,
                v_y: 870.0
            })
        );
        let range_default = VerticalMetrics {
            w1y: -1000.0,
            v_x: 500.0,
            v_y: 880.0,
        };
        assert_eq!(metrics.get(&200), Some(&range_default));
        assert_eq!(metrics.get(&201), Some(&range_default));
        assert_eq!(metrics.get(&202), None);
    }

    /// Missing `/W2` ⇒ `None`. Horizontal-only fonts must skip the HashMap
    /// allocation so they pay no per-glyph lookup cost in the hot path.
    #[test]
    fn test_parse_w2_missing_returns_none() {
        let dict: HashMap<String, Object> = HashMap::new();
        assert!(FontInfo::parse_cid_vertical_metrics(&dict, "Test").is_none());
    }

    /// Empty `/W2` array ⇒ `None`.
    #[test]
    fn test_parse_w2_empty_returns_none() {
        let mut dict: HashMap<String, Object> = HashMap::new();
        dict.insert("W2".to_string(), Object::Array(vec![]));
        assert!(FontInfo::parse_cid_vertical_metrics(&dict, "Test").is_none());
    }

    /// `/W2` accepts real-valued metrics (some writers use floats for
    /// fine-tuned vertical adjustments).
    #[test]
    fn test_parse_w2_real_values() {
        let mut dict: HashMap<String, Object> = HashMap::new();
        dict.insert(
            "W2".to_string(),
            Object::Array(vec![
                Object::Integer(1),
                Object::Integer(1),
                Object::Real(-987.5),
                Object::Real(501.25),
                Object::Real(879.75),
            ]),
        );
        let metrics = FontInfo::parse_cid_vertical_metrics(&dict, "Test").unwrap();
        assert_eq!(
            metrics.get(&1),
            Some(&VerticalMetrics {
                w1y: -987.5,
                v_x: 501.25,
                v_y: 879.75
            })
        );
    }

    /// `/W2` Form A with a malformed inner triple must not desynchronise
    /// the CID assignment of subsequent triples. The original
    /// implementation advanced `j` by 1 on a non-numeric element without
    /// touching `emitted`, so every following triple was shifted up by
    /// one CID. Spec stance: a triple is atomic — drop the whole triple
    /// (advance `j` by 3 and `emitted` by 1) so the CID alignment of the
    /// rest of the inner array is preserved.
    #[test]
    fn test_parse_w2_form_a_skips_malformed_triple_without_desync() {
        let mut dict: HashMap<String, Object> = HashMap::new();
        // CID 10 is intentionally malformed (a name where w1y should be).
        // CID 11 must remain aligned to its proper triple, not slide into
        // CID 10's slot. ~keep
        dict.insert(
            "W2".to_string(),
            Object::Array(vec![
                Object::Integer(10),
                Object::Array(vec![
                    Object::Name("Bogus".to_string()),
                    Object::Integer(500),
                    Object::Integer(880),
                    Object::Integer(-1000),
                    Object::Integer(500),
                    Object::Integer(880),
                ]),
            ]),
        );
        let metrics = FontInfo::parse_cid_vertical_metrics(&dict, "Test").unwrap();
        // CID 10 was malformed: must NOT carry the metrics that belong to CID 11. ~keep
        assert!(
            !metrics.contains_key(&10),
            "malformed CID 10 must not appear in metrics; got {:?}",
            metrics.get(&10)
        );
        // CID 11 must carry its own metrics — not collapsed onto CID 10 or
        // shifted into a different CID slot. ~keep
        assert_eq!(
            metrics.get(&11),
            Some(&VerticalMetrics {
                w1y: -1000.0,
                v_x: 500.0,
                v_y: 880.0
            })
        );
    }

    /// `/W2` Form B near the top of the u16 range must not silently
    /// collapse every overflowing CID onto u16::MAX via saturating
    /// arithmetic. The loop must break (with a warning log) when the
    /// requested range would wrap past 0xFFFF.
    #[test]
    fn test_parse_w2_form_b_overflow_does_not_collapse() {
        // c_first = 0xFFFB, c_last = 0xFFFF — fits exactly within u16 so
        // every CID in 65531..=65535 must be inserted distinctly. A
        // saturating-add bug would collapse them all onto u16::MAX (and
        // an unchecked-add bug would wrap around to 0). ~keep
        let mut dict: HashMap<String, Object> = HashMap::new();
        dict.insert(
            "W2".to_string(),
            Object::Array(vec![
                Object::Integer(0xFFFB),
                Object::Integer(0xFFFF),
                Object::Integer(-1000),
                Object::Integer(500),
                Object::Integer(880),
            ]),
        );
        let metrics = FontInfo::parse_cid_vertical_metrics(&dict, "Test").unwrap();
        let expected = VerticalMetrics {
            w1y: -1000.0,
            v_x: 500.0,
            v_y: 880.0,
        };
        for cid in 0xFFFBu16..=0xFFFFu16 {
            assert_eq!(
                metrics.get(&cid),
                Some(&expected),
                "CID 0x{:04X} must carry the range metrics",
                cid
            );
        }
        assert_eq!(
            metrics.len(),
            5,
            "Form B near u16::MAX should insert 5 distinct entries; got {}",
            metrics.len()
        );
    }

    /// `/W2` Form A with a CID start near u16::MAX and an inner array
    /// long enough to overflow MUST stop emitting on overflow rather than
    /// silently collapsing every subsequent CID onto u16::MAX via
    /// saturating arithmetic.
    #[test]
    fn test_parse_w2_form_a_stops_on_overflow() {
        let mut dict: HashMap<String, Object> = HashMap::new();
        // cid_start = 0xFFFE — only two slots remain (0xFFFE, 0xFFFF) so
        // the third triple would wrap. Confirm we emit exactly two
        // distinct CIDs, not three (which would imply two metrics
        // collapsed onto u16::MAX) and not zero (which would imply a
        // panic-on-overflow bug). ~keep
        dict.insert(
            "W2".to_string(),
            Object::Array(vec![
                Object::Integer(0xFFFE),
                Object::Array(vec![
                    Object::Integer(-1000),
                    Object::Integer(500),
                    Object::Integer(880),
                    Object::Integer(-900),
                    Object::Integer(510),
                    Object::Integer(870),
                    // CID 0x10000 — overflows; must be DROPPED. ~keep
                    Object::Integer(-800),
                    Object::Integer(520),
                    Object::Integer(860),
                ]),
            ]),
        );
        let metrics = FontInfo::parse_cid_vertical_metrics(&dict, "Test").unwrap();
        assert_eq!(
            metrics.get(&0xFFFE),
            Some(&VerticalMetrics {
                w1y: -1000.0,
                v_x: 500.0,
                v_y: 880.0
            })
        );
        assert_eq!(
            metrics.get(&0xFFFF),
            Some(&VerticalMetrics {
                w1y: -900.0,
                v_x: 510.0,
                v_y: 870.0
            })
        );
        assert_eq!(
            metrics.len(),
            2,
            "Form A overflow must drop overflowing triples; got {} entries",
            metrics.len()
        );
    }

    /// `/DW2` overrides only `v_y` and `w1y`; `v_x` always defaults to
    /// `500` per spec (§9.7.4.3 — only two numbers are settable via /DW2).
    #[test]
    fn test_parse_dw2_overrides_defaults() {
        let mut dict: HashMap<String, Object> = HashMap::new();
        dict.insert(
            "DW2".to_string(),
            Object::Array(vec![Object::Integer(850), Object::Integer(-1100)]),
        );
        let dw2 = FontInfo::parse_dw2(&dict);
        assert_eq!(dw2.v_y, 850.0);
        assert_eq!(dw2.w1y, -1100.0);
        assert_eq!(dw2.v_x, 500.0, "v_x is not settable via /DW2");
    }

    /// Missing `/DW2` ⇒ spec defaults `(w1y=-1000, v_x=500, v_y=880)`.
    #[test]
    fn test_parse_dw2_missing_uses_spec_default() {
        let dict: HashMap<String, Object> = HashMap::new();
        assert_eq!(FontInfo::parse_dw2(&dict), VerticalMetrics::SPEC_DEFAULT);
    }

    /// Malformed `/DW2` (single element instead of two) ⇒ spec defaults.
    /// Better to use safe defaults than expose half-parsed metrics that
    /// would shift glyph positions in unpredictable ways.
    #[test]
    fn test_parse_dw2_short_array_uses_spec_default() {
        let mut dict: HashMap<String, Object> = HashMap::new();
        dict.insert("DW2".to_string(), Object::Array(vec![Object::Integer(800)]));
        assert_eq!(FontInfo::parse_dw2(&dict), VerticalMetrics::SPEC_DEFAULT);
    }

    /// `/DW2` with real-valued numbers parses cleanly.
    #[test]
    fn test_parse_dw2_real_values() {
        let mut dict: HashMap<String, Object> = HashMap::new();
        dict.insert(
            "DW2".to_string(),
            Object::Array(vec![Object::Real(875.5), Object::Real(-990.25)]),
        );
        let dw2 = FontInfo::parse_dw2(&dict);
        assert_eq!(dw2.v_y, 875.5);
        assert_eq!(dw2.w1y, -990.25);
        assert_eq!(dw2.v_x, 500.0);
    }

    /// `wmode_from_predefined_cmap_name` returns 1 for any name with a `-V`
    /// suffix and for the bare legacy `V`. This is the cheap fast path that
    /// avoids parsing the encoding CMap stream when we already know the name
    /// declares vertical writing.
    #[test]
    fn test_wmode_from_predefined_cmap_name_vertical() {
        assert_eq!(wmode_from_predefined_cmap_name("Identity-V"), 1);
        assert_eq!(wmode_from_predefined_cmap_name("UniJIS-UTF16-V"), 1);
        assert_eq!(wmode_from_predefined_cmap_name("UniGB-UTF16-V"), 1);
        assert_eq!(wmode_from_predefined_cmap_name("UniCNS-UTF16-V"), 1);
        assert_eq!(wmode_from_predefined_cmap_name("UniKS-UTF16-V"), 1);
        assert_eq!(wmode_from_predefined_cmap_name("GBK-EUC-V"), 1);
        assert_eq!(wmode_from_predefined_cmap_name("90ms-RKSJ-V"), 1);
        assert_eq!(wmode_from_predefined_cmap_name("V"), 1);
    }

    /// Horizontal-mode names (the overwhelming majority) must return 0 so
    /// the wmode flag stays cold for normal documents.
    #[test]
    fn test_wmode_from_predefined_cmap_name_horizontal() {
        assert_eq!(wmode_from_predefined_cmap_name("Identity-H"), 0);
        assert_eq!(wmode_from_predefined_cmap_name("UniJIS-UTF16-H"), 0);
        assert_eq!(wmode_from_predefined_cmap_name("UniGB-UTF16-H"), 0);
        assert_eq!(wmode_from_predefined_cmap_name("H"), 0);
        assert_eq!(wmode_from_predefined_cmap_name("WinAnsiEncoding"), 0);
        assert_eq!(wmode_from_predefined_cmap_name("MacRomanEncoding"), 0);
        assert_eq!(wmode_from_predefined_cmap_name("Adobe-Japan1-6"), 0);
        // Edge case: the substring `-V` appears inside but not as a suffix. ~keep
        assert_eq!(wmode_from_predefined_cmap_name("V-foo"), 0);
        assert_eq!(wmode_from_predefined_cmap_name("Volt"), 0);
    }

    /// `FontInfo::get_vertical_metrics` returns per-CID metrics when
    /// available, falls back to `/DW2` defaults otherwise. This is the
    /// accessor the rasterizer and extractor call on the hot path of every
    /// vertical-mode glyph.
    #[test]
    fn test_get_vertical_metrics_lookup_precedence() {
        let mut per_cid: HashMap<u16, VerticalMetrics> = HashMap::new();
        per_cid.insert(
            7,
            VerticalMetrics {
                w1y: -900.0,
                v_x: 480.0,
                v_y: 870.0,
            },
        );

        let font = make_font(|f| {
            f.subtype = "Type0".to_string();
            f.cid_vertical_metrics = Some(per_cid);
            f.cid_default_vertical_metrics = VerticalMetrics {
                w1y: -1050.0,
                v_x: 500.0,
                v_y: 900.0,
            };
        });

        assert_eq!(
            font.get_vertical_metrics(7),
            VerticalMetrics {
                w1y: -900.0,
                v_x: 480.0,
                v_y: 870.0
            }
        );
        assert_eq!(
            font.get_vertical_metrics(99),
            VerticalMetrics {
                w1y: -1050.0,
                v_x: 500.0,
                v_y: 900.0
            }
        );
    }

    /// When neither `/W2` nor `/DW2` is parsed, `get_vertical_metrics`
    /// returns the spec defaults — keeping rendering correct for the common
    /// case of a CIDFont that ships only horizontal metrics but is used in
    /// a vertical context (caller has already established wmode=1 by name).
    #[test]
    fn test_get_vertical_metrics_spec_default_fallback() {
        let font = make_font(|f| {
            f.subtype = "Type0".to_string();
            f.cid_vertical_metrics = None;
            f.cid_default_vertical_metrics = VerticalMetrics::SPEC_DEFAULT;
        });
        assert_eq!(font.get_vertical_metrics(0x4E00), VerticalMetrics::SPEC_DEFAULT);
        assert_eq!(VerticalMetrics::SPEC_DEFAULT.w1y, -1000.0);
        assert_eq!(VerticalMetrics::SPEC_DEFAULT.v_x, 500.0);
        assert_eq!(VerticalMetrics::SPEC_DEFAULT.v_y, 880.0);
    }

    #[test]
    fn test_shift_jis_single_byte_ascii() {
        assert_eq!(shift_jis_to_unicode(0x41), Some('A'));
        assert_eq!(shift_jis_to_unicode(0x20), Some(' '));
    }

    #[test]
    fn test_shift_jis_two_byte_katakana() {
        assert_eq!(shift_jis_to_unicode(0x8341), Some('ア'));
    }

    #[test]
    fn test_shift_jis_invalid() {
        assert_eq!(shift_jis_to_unicode(0xFFFF), None);
    }

    #[test]
    fn test_standard_encoding_lookup_standard_encoding_ascii() {
        assert_eq!(
            standard_encoding_lookup("StandardEncoding", b'A'),
            Some("A".to_string())
        );
        assert_eq!(
            standard_encoding_lookup("StandardEncoding", b' '),
            Some(" ".to_string())
        );
    }

    #[test]
    fn test_standard_encoding_lookup_standard_encoding_extended() {
        assert_eq!(
            standard_encoding_lookup("StandardEncoding", 0xAE),
            Some("\u{FB01}".to_string())
        );
        assert_eq!(
            standard_encoding_lookup("StandardEncoding", 0xD0),
            Some("\u{2014}".to_string())
        );
        assert_eq!(
            standard_encoding_lookup("StandardEncoding", 0xA1),
            Some("\u{00A1}".to_string())
        );
    }

    #[test]
    fn test_standard_encoding_lookup_standard_encoding_unmapped() {
        assert_eq!(standard_encoding_lookup("StandardEncoding", 0x00), None);
        assert_eq!(standard_encoding_lookup("StandardEncoding", 0xB0), None);
    }

    #[test]
    fn test_standard_encoding_lookup_macroman_ascii() {
        assert_eq!(
            standard_encoding_lookup("MacRomanEncoding", b'Z'),
            Some("Z".to_string())
        );
    }

    #[test]
    fn test_standard_encoding_lookup_macroman_extended() {
        assert_eq!(
            standard_encoding_lookup("MacRomanEncoding", 0x80),
            Some("\u{00C4}".to_string())
        );
        assert_eq!(
            standard_encoding_lookup("MacRomanEncoding", 0xD0),
            Some("\u{2013}".to_string())
        );
        assert_eq!(
            standard_encoding_lookup("MacRomanEncoding", 0xCA),
            Some("\u{00A0}".to_string())
        );
        assert_eq!(
            standard_encoding_lookup("MacRomanEncoding", 0xF0),
            Some("\u{F8FF}".to_string())
        );
    }

    #[test]
    fn test_standard_encoding_lookup_macroman_unmapped() {
        assert_eq!(standard_encoding_lookup("MacRomanEncoding", 0x00), None);
    }

    #[test]
    fn test_standard_encoding_lookup_winansi_extended() {
        assert_eq!(
            standard_encoding_lookup("WinAnsiEncoding", 0x80),
            Some("\u{20AC}".to_string())
        );
        assert_eq!(
            standard_encoding_lookup("WinAnsiEncoding", 0x96),
            Some("\u{2013}".to_string())
        );
        assert_eq!(
            standard_encoding_lookup("WinAnsiEncoding", 0xA0),
            Some("\u{00A0}".to_string())
        );
    }

    #[test]
    fn test_standard_encoding_lookup_winansi_undefined_holes() {
        assert_eq!(standard_encoding_lookup("WinAnsiEncoding", 0x81), None);
        assert_eq!(standard_encoding_lookup("WinAnsiEncoding", 0x8D), None);
    }

    #[test]
    fn test_standard_encoding_lookup_pdfdoc() {
        assert_eq!(
            standard_encoding_lookup("PDFDocEncoding", 0x80),
            Some("\u{2022}".to_string())
        );
        assert_eq!(
            standard_encoding_lookup("PDFDocEncoding", 0x84),
            Some("\u{2014}".to_string())
        );
        assert_eq!(standard_encoding_lookup("PDFDocEncoding", b'B'), Some("B".to_string()));
    }

    #[test]
    fn test_standard_encoding_lookup_unknown_encoding() {
        assert_eq!(
            standard_encoding_lookup("SomeWeirdEncoding", b'X'),
            Some("X".to_string())
        );
        assert_eq!(standard_encoding_lookup("SomeWeirdEncoding", 0x01), None);
        assert_eq!(standard_encoding_lookup("SomeWeirdEncoding", 0x80), None);
    }

    #[test]
    fn test_pdfdoc_encoding_ascii_range() {
        assert_eq!(pdfdoc_encoding_lookup(0x00), Some('\0'));
        assert_eq!(pdfdoc_encoding_lookup(0x41), Some('A'));
        assert_eq!(pdfdoc_encoding_lookup(0x7F), Some('\x7F'));
    }

    #[test]
    fn test_pdfdoc_encoding_special_range() {
        assert_eq!(pdfdoc_encoding_lookup(0x80), Some('\u{2022}'));
        assert_eq!(pdfdoc_encoding_lookup(0x85), Some('\u{2013}'));
        assert_eq!(pdfdoc_encoding_lookup(0x93), Some('\u{FB01}'));
        assert_eq!(pdfdoc_encoding_lookup(0x94), Some('\u{FB02}'));
        assert_eq!(pdfdoc_encoding_lookup(0x92), Some('\u{2122}'));
    }

    #[test]
    fn test_pdfdoc_encoding_undefined() {
        assert_eq!(pdfdoc_encoding_lookup(0x9F), None);
    }

    #[test]
    fn test_pdfdoc_encoding_latin1_range() {
        assert_eq!(pdfdoc_encoding_lookup(0xA0), Some('\u{00A0}'));
        assert_eq!(pdfdoc_encoding_lookup(0xFF), Some('\u{00FF}'));
    }

    #[test]
    fn test_symbol_encoding_greek_lowercase() {
        assert_eq!(symbol_encoding_lookup(0x61), Some('α'));
        assert_eq!(symbol_encoding_lookup(0x62), Some('β'));
        assert_eq!(symbol_encoding_lookup(0x67), Some('γ'));
        assert_eq!(symbol_encoding_lookup(0x72), Some('ρ'));
        assert_eq!(symbol_encoding_lookup(0x77), Some('ω'));
    }

    #[test]
    fn test_symbol_encoding_greek_uppercase() {
        assert_eq!(symbol_encoding_lookup(0x44), Some('Δ'));
        assert_eq!(symbol_encoding_lookup(0x53), Some('Σ'));
        assert_eq!(symbol_encoding_lookup(0x57), Some('Ω'));
    }

    #[test]
    fn test_symbol_encoding_math_operators() {
        assert_eq!(symbol_encoding_lookup(0xE1), Some('∑'));
        assert_eq!(symbol_encoding_lookup(0xF2), Some('∫'));
        assert_eq!(symbol_encoding_lookup(0xD6), Some('√'));
        assert_eq!(symbol_encoding_lookup(0xB1), Some('±'));
        assert_eq!(symbol_encoding_lookup(0xB9), Some('≠'));
    }

    #[test]
    fn test_symbol_encoding_digits() {
        assert_eq!(symbol_encoding_lookup(0x30), Some('0'));
        assert_eq!(symbol_encoding_lookup(0x39), Some('9'));
    }

    #[test]
    fn test_symbol_encoding_punctuation() {
        assert_eq!(symbol_encoding_lookup(0x20), Some(' '));
        assert_eq!(symbol_encoding_lookup(0x2B), Some('+'));
        assert_eq!(symbol_encoding_lookup(0x2D), Some('−'));
    }

    #[test]
    fn test_symbol_encoding_unmapped() {
        assert_eq!(symbol_encoding_lookup(0x00), None);
        assert_eq!(symbol_encoding_lookup(0x01), None);
    }

    #[test]
    fn test_zapf_dingbats_common() {
        assert_eq!(zapf_dingbats_encoding_lookup(0x20), Some(' '));
        assert_eq!(zapf_dingbats_encoding_lookup(0x21), Some('✁'));
        assert_eq!(zapf_dingbats_encoding_lookup(0x33), Some('✓'));
        assert_eq!(zapf_dingbats_encoding_lookup(0x34), Some('✔'));
        assert_eq!(zapf_dingbats_encoding_lookup(0x48), Some('★'));
    }

    #[test]
    fn test_zapf_dingbats_geometric() {
        assert_eq!(zapf_dingbats_encoding_lookup(0x6C), Some('●'));
        assert_eq!(zapf_dingbats_encoding_lookup(0x6F), Some('■'));
    }

    /// ZapfDingbats circled-digit ranges (Annex D.6); codes in hex of the
    /// spec's octal CODE column.
    #[test]
    fn test_zapf_dingbats_circled_digits() {
        assert_eq!(zapf_dingbats_encoding_lookup(0xAC), Some('\u{2460}'));
        assert_eq!(zapf_dingbats_encoding_lookup(0xB5), Some('\u{2469}'));
        assert_eq!(zapf_dingbats_encoding_lookup(0xB6), Some('\u{2776}'));
        assert_eq!(zapf_dingbats_encoding_lookup(0xBF), Some('\u{277F}'));
        assert_eq!(zapf_dingbats_encoding_lookup(0xC0), Some('\u{2780}'));
        assert_eq!(zapf_dingbats_encoding_lookup(0xC9), Some('\u{2789}'));
        assert_eq!(zapf_dingbats_encoding_lookup(0xCA), Some('\u{278A}'));
        assert_eq!(zapf_dingbats_encoding_lookup(0xD3), Some('\u{2793}'));
    }

    /// ZapfDingbats arrow ranges (Annex D.6, octal 324–376).
    #[test]
    fn test_zapf_dingbats_arrows() {
        assert_eq!(zapf_dingbats_encoding_lookup(0xD4), Some('\u{2794}'));
        assert_eq!(zapf_dingbats_encoding_lookup(0xD5), Some('\u{2192}'));
        assert_eq!(zapf_dingbats_encoding_lookup(0xD8), Some('\u{2798}'));
        assert_eq!(zapf_dingbats_encoding_lookup(0xEF), Some('\u{27AF}'));
        assert_eq!(zapf_dingbats_encoding_lookup(0xF0), None);
        assert_eq!(zapf_dingbats_encoding_lookup(0xF1), Some('\u{27B1}'));
        assert_eq!(zapf_dingbats_encoding_lookup(0xFE), Some('\u{27BE}'));
    }

    #[test]
    fn test_zapf_dingbats_unmapped() {
        assert_eq!(zapf_dingbats_encoding_lookup(0x00), None);
        assert_eq!(zapf_dingbats_encoding_lookup(0xFF), None);
    }

    #[test]
    fn test_glyph_name_to_unicode_tex_math() {
        assert_eq!(glyph_name_to_unicode("square"), Some('\u{25A1}'));
        assert_eq!(glyph_name_to_unicode("emptyset"), Some('\u{2205}'));
        assert_eq!(glyph_name_to_unicode("infty"), Some('\u{221E}'));
        assert_eq!(glyph_name_to_unicode("nabla"), Some('\u{2207}'));
        assert_eq!(glyph_name_to_unicode("forall"), Some('\u{2200}'));
        assert_eq!(glyph_name_to_unicode("checkmark"), Some('\u{2713}'));
    }

    #[test]
    fn test_glyph_name_to_unicode_underscore_compound() {
        assert_eq!(glyph_name_to_unicode("f_f"), Some('f'));
        assert_eq!(glyph_name_to_unicode("T_h"), Some('T'));
    }

    #[test]
    fn test_glyph_name_to_unicode_uni_format_edge_cases() {
        assert_eq!(glyph_name_to_unicode("uni004"), None);
        assert_eq!(glyph_name_to_unicode("uniZZZZ"), None);
    }

    #[test]
    fn test_glyph_name_to_unicode_u_format_long() {
        assert_eq!(glyph_name_to_unicode("u1F600"), Some('\u{1F600}'));
    }

    #[test]
    fn test_glyph_name_to_unicode_string_simple() {
        assert_eq!(glyph_name_to_unicode_string("A"), Some("A".to_string()));
    }

    #[test]
    fn test_glyph_name_to_unicode_string_compound_ff() {
        // glyph_name_to_unicode("f_f") returns Some('f') — first component via AGL
        // So glyph_name_to_unicode_string wraps it as "f" (single-char short-circuit) ~keep
        assert_eq!(glyph_name_to_unicode_string("f_f"), Some("f".to_string()));
    }

    #[test]
    fn test_glyph_name_to_unicode_string_compound_all_known() {
        assert_eq!(glyph_name_to_unicode_string("T_h"), Some("T".to_string()));
    }

    #[test]
    fn test_glyph_name_to_unicode_string_compound_unknown_part() {
        // "f_unknownglyph" — glyph_name_to_unicode finds 'f' (first component via underscore rule)
        // So it returns Some("f") not None ~keep
        assert_eq!(glyph_name_to_unicode_string("f_unknownglyph"), Some("f".to_string()));
    }

    #[test]
    fn test_glyph_name_to_unicode_string_totally_unknown_compound() {
        assert_eq!(glyph_name_to_unicode_string("xyzzy_plugh"), None);
    }

    #[test]
    fn test_glyph_name_to_unicode_string_unknown() {
        assert_eq!(glyph_name_to_unicode_string("totallyunknown"), None);
    }

    // =========================================================================
    // Unified AGL fallback chain
    //
    // A robust ToUnicode + embedded-cmap + AGL fallback chain lives in
    // `src/fonts/character_mapper.rs::glyph_name_to_unicode`, but the original
    // full-document Type0 / Identity-H call site at
    // `font_dict.rs::Font::char_code_to_unicode` was the only consumer. Simple
    // fonts, Type1 / CFF embedded encodings, and `/Differences` arrays still
    // routed through this `font_dict::glyph_name_to_unicode` entry, which
    // lacked the newer chain's variant-suffix stripping (`.alt`, `.sc`,
    // `.001`). delegates to the unified chain as a final fallback so
    // all callers — including any future inline-image font-resolution path
    // (PDF spec §8.9.7) — share the same behaviour.
    // ========================================================================= ~keep

    #[test]
    fn glyph_name_with_variant_suffix_resolves_via_unified_chain() {
        // Subset fonts append stylistic-variant tags (`.sc`, `.alt`, `.001`)
        // to the canonical glyph name. The chain strips the suffix
        // returns the base codepoint; this entry now picks that up too. ~keep
        assert_eq!(glyph_name_to_unicode("A.sc"), Some('A'));
        assert_eq!(glyph_name_to_unicode("bullet.alt"), Some('\u{2022}'));
        assert_eq!(glyph_name_to_unicode("fi.001"), Some('\u{FB01}'));
        assert_eq!(glyph_name_to_unicode("xyzzy.sc"), None);
    }

    #[test]
    fn glyph_name_string_with_variant_suffix_resolves_via_unified_chain() {
        assert_eq!(glyph_name_to_unicode_string("A.sc"), Some("A".to_string()));
        assert_eq!(glyph_name_to_unicode_string("bullet.alt"), Some("\u{2022}".to_string()));
        assert_eq!(glyph_name_to_unicode_string("fi.001"), Some("\u{FB01}".to_string()));
    }

    #[test]
    fn unified_chain_does_not_regress_existing_lookups() {
        assert_eq!(glyph_name_to_unicode("A"), Some('A'));
        assert_eq!(glyph_name_to_unicode("space"), Some(' '));
        assert_eq!(glyph_name_to_unicode("bullet"), Some('\u{2022}'));
        assert_eq!(glyph_name_to_unicode("fi"), Some('\u{FB01}'));
        assert_eq!(glyph_name_to_unicode("uni2022"), Some('\u{2022}'));
        assert_eq!(glyph_name_to_unicode("u1F600"), Some('\u{1F600}'));
        assert_eq!(glyph_name_to_unicode("totallyunknown"), None);
    }

    #[test]
    fn test_is_ligature_char_all_variants() {
        assert!(is_ligature_char('\u{FB00}'));
        assert!(is_ligature_char('\u{FB01}'));
        assert!(is_ligature_char('\u{FB02}'));
        assert!(is_ligature_char('\u{FB03}'));
        assert!(is_ligature_char('\u{FB04}'));
        assert!(is_ligature_char('\u{FB05}'));
        assert!(is_ligature_char('\u{FB06}'));
        assert!(!is_ligature_char('A'));
        assert!(!is_ligature_char(' '));
    }

    #[test]
    fn test_expand_ligature_char_all_variants() {
        assert_eq!(expand_ligature_char('\u{FB00}'), Some("ff"));
        assert_eq!(expand_ligature_char('\u{FB01}'), Some("fi"));
        assert_eq!(expand_ligature_char('\u{FB02}'), Some("fl"));
        assert_eq!(expand_ligature_char('\u{FB03}'), Some("ffi"));
        assert_eq!(expand_ligature_char('\u{FB04}'), Some("ffl"));
        assert_eq!(expand_ligature_char('\u{FB05}'), Some("st"));
        assert_eq!(expand_ligature_char('\u{FB06}'), Some("st"));
        assert_eq!(expand_ligature_char('x'), None);
    }

    #[test]
    fn test_get_glyph_width_simple_font_widths_array() {
        let font = make_font(|f| {
            f.widths = Some(vec![200.0, 300.0, 400.0, 500.0]);
            f.first_char = Some(65);
            f.last_char = Some(68);
            f.default_width = 600.0;
        });
        assert_eq!(font.get_glyph_width(65), 200.0);
        assert_eq!(font.get_glyph_width(66), 300.0);
        assert_eq!(font.get_glyph_width(67), 400.0);
        assert_eq!(font.get_glyph_width(68), 500.0);
        assert_eq!(font.get_glyph_width(64), 600.0);
        assert_eq!(font.get_glyph_width(69), 600.0);
    }

    #[test]
    fn test_get_glyph_width_below_first_char() {
        let font = make_font(|f| {
            f.widths = Some(vec![250.0]);
            f.first_char = Some(100);
            f.last_char = Some(100);
            f.default_width = 777.0;
        });
        assert_eq!(font.get_glyph_width(50), 777.0);
    }

    #[test]
    fn test_get_glyph_width_no_widths_no_cid() {
        let font = make_font(|f| {
            f.default_width = 550.0;
        });
        assert_eq!(font.get_glyph_width(65), 550.0);
    }

    #[test]
    fn test_get_space_glyph_width_from_array() {
        let font = make_font(|f| {
            f.widths = Some(vec![250.0]);
            f.first_char = Some(32);
            f.last_char = Some(32);
        });
        assert_eq!(font.get_space_glyph_width(), 250.0);
    }

    #[test]
    fn test_get_space_glyph_width_default() {
        let font = make_font(|f| {
            f.default_width = 333.0;
        });
        assert_eq!(font.get_space_glyph_width(), 333.0);
    }

    #[test]
    fn test_normalize_cjk_radical_forms() {
        assert_eq!(normalize_cjk_radical_forms("⽋点"), "欠点");
        assert_eq!(normalize_cjk_radical_forms("⽴⾮⾔⾦"), "立非言金");
        // CJK Radicals Supplement (U+2E80–2EFF) → unified ideograph. NFKC has
        // no decomposition for these, which is why the mapping is Unicode's
        // Equivalent_Unified_Ideograph property instead. ~keep
        assert_eq!(normalize_cjk_radical_forms("\u{2ED8}"), "青");
        assert_eq!(normalize_cjk_radical_forms("\u{2EEB}"), "斉");
        assert_eq!(normalize_cjk_radical_forms("\u{2EC4}空"), "西空");
        assert_eq!(normalize_cjk_radical_forms("\u{2E80}"), "\u{2E80}");
        assert_eq!(normalize_cjk_radical_forms("実⽴確率"), "実立確率");
        assert_eq!(normalize_cjk_radical_forms("欠点０１２"), "欠点０１２");
        assert_eq!(normalize_cjk_radical_forms("hello"), "hello");
    }

    #[test]
    fn test_is_symbolic_flag_set() {
        let font = make_font(|f| {
            f.flags = Some(0x04);
        });
        assert!(font.is_symbolic());
    }

    #[test]
    fn test_is_symbolic_flag_not_set() {
        let font = make_font(|f| {
            f.flags = Some(0x20);
        });
        assert!(!font.is_symbolic());
    }

    #[test]
    fn test_is_symbolic_no_flags_symbol_name() {
        let font = make_font(|f| {
            f.base_font = "Symbol".to_string();
        });
        assert!(font.is_symbolic());
    }

    #[test]
    fn test_is_symbolic_no_flags_zapf_name() {
        let font = make_font(|f| {
            f.base_font = "ZapfDingbats".to_string();
        });
        assert!(font.is_symbolic());
    }

    #[test]
    fn test_is_symbolic_no_flags_normal_name() {
        let font = make_font(|f| {
            f.base_font = "Helvetica".to_string();
        });
        assert!(!font.is_symbolic());
    }

    #[test]
    fn test_get_encoded_char_custom() {
        let mut map = HashMap::new();
        map.insert(0x41, 'X');
        map.insert(0x42, 'Y');
        let font = make_font(|f| {
            f.encoding = Encoding::Custom(map);
        });
        assert_eq!(font.get_encoded_char(0x41), Some('X'));
        assert_eq!(font.get_encoded_char(0x42), Some('Y'));
        assert_eq!(font.get_encoded_char(0x43), None);
    }

    #[test]
    fn test_get_encoded_char_standard_ascii() {
        let font = make_font(|f| {
            f.encoding = Encoding::Standard("WinAnsiEncoding".to_string());
        });
        assert_eq!(font.get_encoded_char(0x41), Some('A'));
        assert_eq!(font.get_encoded_char(0x20), Some(' '));
        assert_eq!(font.get_encoded_char(0x80), None);
    }

    #[test]
    fn test_get_encoded_char_identity_ascii() {
        let font = make_font(|f| {
            f.encoding = Encoding::Identity;
        });
        assert_eq!(font.get_encoded_char(0x41), Some('A'));
        assert_eq!(font.get_encoded_char(0x80), None);
    }

    #[test]
    fn test_has_custom_encoding_true() {
        let font = make_font(|f| {
            f.encoding = Encoding::Custom(HashMap::new());
        });
        assert!(font.has_custom_encoding());
    }

    #[test]
    fn test_has_custom_encoding_false_standard() {
        let font = make_font(|_| {});
        assert!(!font.has_custom_encoding());
    }

    #[test]
    fn test_has_custom_encoding_false_identity() {
        let font = make_font(|f| {
            f.encoding = Encoding::Identity;
        });
        assert!(!font.has_custom_encoding());
    }

    #[test]
    fn test_char_to_unicode_symbol_font() {
        let font = make_font(|f| {
            f.base_font = "Symbol".to_string();
            f.flags = Some(0x04);
            f.encoding = Encoding::Standard("SymbolicBuiltIn".to_string());
        });
        assert_eq!(font.char_to_unicode(0x61), Some("α".to_string()));
        assert_eq!(font.char_to_unicode(0x53), Some("Σ".to_string()));
        assert_eq!(font.char_to_unicode(0xF2), Some("∫".to_string()));
    }

    #[test]
    fn test_char_to_unicode_zapfdingbats_font() {
        let font = make_font(|f| {
            f.base_font = "ZapfDingbats".to_string();
            f.flags = Some(0x04);
            f.encoding = Encoding::Standard("SymbolicBuiltIn".to_string());
        });
        assert_eq!(font.char_to_unicode(0x33), Some("✓".to_string()));
        assert_eq!(font.char_to_unicode(0x48), Some("★".to_string()));
    }

    #[test]
    fn test_char_to_unicode_ligature_fallback_expansion() {
        let font = make_font(|f| {
            f.encoding = Encoding::Standard("WinAnsiEncoding".to_string());
        });
        assert_eq!(font.char_to_unicode(0xFB01), Some("fi".to_string()));
        assert_eq!(font.char_to_unicode(0xFB03), Some("ffi".to_string()));
    }

    #[test]
    fn test_char_to_unicode_custom_encoding_with_ligature() {
        let mut custom = HashMap::new();
        custom.insert(0x01, '\u{FB01}');
        let font = make_font(|f| {
            f.encoding = Encoding::Custom(custom);
        });
        assert_eq!(font.char_to_unicode(0x01), Some("fi".to_string()));
    }

    #[test]
    fn test_char_to_unicode_custom_encoding_multi_char_map() {
        let font = make_font(|f| {
            f.encoding = Encoding::Custom(HashMap::new());
            f.multi_char_map.insert(0x01, "ff".to_string());
        });
        assert_eq!(font.char_to_unicode(0x01), Some("ff".to_string()));
    }

    #[test]
    fn test_char_to_unicode_tounicode_fffd_fallback() {
        // A ToUnicode mapping to U+FFFD means the font author explicitly declared
        // "no Unicode equivalent" for this code. Per Fix B (§9.10.2) the function
        // must return U+FFFD and NOT fall through to the encoding-based path. ~keep
        let cmap_data = b"beginbfchar\n<0041> <FFFD>\nendbfchar";
        let font = make_font(|f| {
            f.to_unicode = Some(LazyCMap::new(cmap_data.to_vec()));
            f.encoding = Encoding::Standard("WinAnsiEncoding".to_string());
        });
        assert_eq!(font.char_to_unicode(0x41), Some("\u{FFFD}".to_string()));
    }

    #[test]
    fn test_char_to_unicode_tounicode_control_char_fallback() {
        // A ToUnicode mapping to a C0 control character is filtered by Fix B.
        // The function must return U+FFFD and NOT fall through to the encoding. ~keep
        let cmap_data = b"beginbfchar\n<0041> <0001>\nendbfchar";
        let font = make_font(|f| {
            f.to_unicode = Some(LazyCMap::new(cmap_data.to_vec()));
            f.encoding = Encoding::Standard("WinAnsiEncoding".to_string());
        });
        assert_eq!(font.char_to_unicode(0x41), Some("\u{FFFD}".to_string()));
    }

    #[test]
    fn test_char_to_unicode_type0_identity_h_with_sysinfo() {
        let font = make_font(|f| {
            f.base_font = "CIDFont+F1".to_string();
            f.subtype = "Type0".to_string();
            f.encoding = Encoding::Standard("Identity-H".to_string());
            f.cid_system_info = Some(CIDSystemInfo {
                registry: "Adobe".to_string(),
                ordering: "Identity".to_string(),
                supplement: 0,
            });
            f.cid_to_gid_map = Some(CIDToGIDMap::Identity);
        });
        assert_eq!(font.char_to_unicode(0x41), Some("A".to_string()));
        assert_eq!(font.char_to_unicode(0x4E2D), Some("\u{4E2D}".to_string()));
    }

    #[test]
    fn test_char_to_unicode_type0_identity_h_no_sysinfo() {
        let font = make_font(|f| {
            f.base_font = "CIDFont+F2".to_string();
            f.subtype = "Type0".to_string();
            f.encoding = Encoding::Standard("Identity-H".to_string());
        });
        assert_eq!(font.char_to_unicode(0x42), Some("B".to_string()));
    }

    #[test]
    fn test_char_to_unicode_type0_identity_encoding_cid_as_unicode() {
        let font = make_font(|f| {
            f.base_font = "MyCIDFont".to_string();
            f.subtype = "Type0".to_string();
            f.encoding = Encoding::Identity;
        });
        assert_eq!(font.char_to_unicode(0x41), Some("A".to_string()));
    }

    #[test]
    fn test_char_to_unicode_type0_identity_encoding_control_char() {
        let font = make_font(|f| {
            f.subtype = "Type0".to_string();
            f.encoding = Encoding::Identity;
        });
        let result = font.char_to_unicode(0x01);
        assert_eq!(result, Some("\u{FFFD}".to_string()));
    }

    #[test]
    fn test_char_to_unicode_simple_font_identity() {
        let font = make_font(|f| {
            f.subtype = "Type1".to_string();
            f.encoding = Encoding::Identity;
        });
        assert_eq!(font.char_to_unicode(0x41), Some("A".to_string()));
        assert_eq!(font.char_to_unicode(0x263A), Some("☺".to_string()));
    }

    #[test]
    fn test_char_to_unicode_truetype_standard_encoding_ascii() {
        let font = make_font(|f| {
            f.subtype = "TrueType".to_string();
            f.encoding = Encoding::Standard("StandardEncoding".to_string());
        });
        assert_eq!(font.char_to_unicode(0x41), Some("A".to_string()));
    }

    #[test]
    fn test_char_to_unicode_macroman_extended() {
        let font = make_font(|f| {
            f.encoding = Encoding::Standard("MacRomanEncoding".to_string());
        });
        assert_eq!(font.char_to_unicode(0x41), Some("A".to_string()));
        assert_eq!(font.char_to_unicode(0x80), Some("\u{00C4}".to_string()));
    }

    #[test]
    fn test_get_font_weight_demibold() {
        let font = make_font(|f| {
            f.base_font = "MyFont-DemiBold".to_string();
        });
        assert_eq!(font.get_font_weight(), FontWeight::SemiBold);
    }

    #[test]
    fn test_get_font_weight_heavy() {
        let font = make_font(|f| {
            f.base_font = "MyFont-Heavy".to_string();
        });
        assert_eq!(font.get_font_weight(), FontWeight::Black);
    }

    #[test]
    fn test_get_font_weight_ultrabold() {
        let font = make_font(|f| {
            f.base_font = "MyFont-UltraBold".to_string();
        });
        assert_eq!(font.get_font_weight(), FontWeight::ExtraBold);
    }

    #[test]
    fn test_get_font_weight_ultralight() {
        let font = make_font(|f| {
            f.base_font = "MyFont-UltraLight".to_string();
        });
        assert_eq!(font.get_font_weight(), FontWeight::ExtraLight);
    }

    #[test]
    fn test_get_byte_to_char_table_basic() {
        let font = make_font(|f| {
            f.encoding = Encoding::Standard("WinAnsiEncoding".to_string());
        });
        let table = font.get_byte_to_char_table();
        assert_eq!(table[0x41], 'A');
        assert_eq!(table[0x20], ' ');
        assert_eq!(table[0x01], '\0');
    }

    #[test]
    fn test_get_byte_to_char_table_tab_newline_passthrough() {
        let font = make_font(|f| {
            let mut custom = HashMap::new();
            custom.insert(0x09u8, '\t');
            custom.insert(0x0Au8, '\n');
            custom.insert(0x0Du8, '\r');
            f.encoding = Encoding::Custom(custom);
        });
        let table = font.get_byte_to_char_table();
        assert_eq!(table[0x09], '\t');
        assert_eq!(table[0x0A], '\n');
        assert_eq!(table[0x0D], '\r');
    }

    #[test]
    fn test_get_byte_to_width_table_basic() {
        let font = make_font(|f| {
            f.widths = Some(vec![200.0, 300.0, 400.0]);
            f.first_char = Some(65);
            f.default_width = 500.0;
        });
        let table = font.get_byte_to_width_table();
        assert_eq!(table[65], 200.0);
        assert_eq!(table[66], 300.0);
        assert_eq!(table[67], 400.0);
        assert_eq!(table[0], 500.0);
        assert_eq!(table[100], 500.0);
    }

    #[test]
    fn test_public_byte_width_table_preserves_authored_zero() {
        let font = make_font(|font| {
            font.base_font = "Helvetica".to_string();
            font.subtype = "Type1".to_string();
            font.widths = Some(vec![0.0]);
            font.first_char = Some(65);
            font.last_char = Some(65);
        });
        assert_eq!(font.get_glyph_width(65), 0.0);
        assert_eq!(font.get_byte_to_width_table()[65], 0.0);
    }

    #[test]
    fn test_get_byte_to_width_table_no_widths() {
        let font = make_font(|f| {
            f.default_width = 600.0;
        });
        let table = font.get_byte_to_width_table();
        for &w in table.iter() {
            assert_eq!(w, 600.0);
        }
    }

    #[test]
    fn test_lookup_predefined_cmap_ordering_fallback_gb1() {
        let sysinfo = Some(CIDSystemInfo {
            registry: "Adobe".to_string(),
            ordering: "GB1".to_string(),
            supplement: 2,
        });
        assert_eq!(lookup_predefined_cmap("SomeCustomCMap", &sysinfo, 34), Some(0x41));
    }

    #[test]
    fn test_lookup_predefined_cmap_ordering_fallback_japan1() {
        let sysinfo = Some(CIDSystemInfo {
            registry: "Adobe".to_string(),
            ordering: "Japan1".to_string(),
            supplement: 4,
        });
        assert_eq!(lookup_predefined_cmap("CustomJapanCMap", &sysinfo, 34), Some(0x41));
    }

    #[test]
    fn test_lookup_predefined_cmap_ordering_fallback_cns1() {
        let sysinfo = Some(CIDSystemInfo {
            registry: "Adobe".to_string(),
            ordering: "CNS1".to_string(),
            supplement: 3,
        });
        assert_eq!(lookup_predefined_cmap("CustomCNSCMap", &sysinfo, 34), Some(0x41));
    }

    #[test]
    fn test_lookup_predefined_cmap_ordering_fallback_korea1() {
        let sysinfo = Some(CIDSystemInfo {
            registry: "Adobe".to_string(),
            ordering: "Korea1".to_string(),
            supplement: 1,
        });
        assert_eq!(lookup_predefined_cmap("CustomKoreaCMap", &sysinfo, 34), Some(0x41));
    }

    #[test]
    fn test_lookup_predefined_cmap_unknown_ordering() {
        let sysinfo = Some(CIDSystemInfo {
            registry: "Custom".to_string(),
            ordering: "Unknown".to_string(),
            supplement: 0,
        });
        assert_eq!(lookup_predefined_cmap("AnyCMap", &sysinfo, 34), None);
    }

    #[test]
    fn test_truetype_cmap_not_truetype() {
        let font = make_font(|f| {
            f.is_truetype_font = false;
            f.embedded_font_data = None;
        });
        assert!(font.truetype_cmap().is_none());
    }

    #[test]
    fn test_truetype_cmap_truetype_no_data() {
        let font = make_font(|f| {
            f.is_truetype_font = true;
            f.embedded_font_data = None;
        });
        assert!(font.truetype_cmap().is_none());
    }

    #[test]
    fn test_truetype_cmap_truetype_empty_data() {
        let font = make_font(|f| {
            f.is_truetype_font = true;
            f.embedded_font_data = Some(Arc::new(vec![]));
        });
        assert!(font.truetype_cmap().is_none());
    }

    #[test]
    fn test_truetype_cmap_truetype_invalid_data() {
        let font = make_font(|f| {
            f.is_truetype_font = true;
            f.embedded_font_data = Some(Arc::new(vec![0xFF, 0xFF, 0xFF, 0xFF]));
        });
        assert!(font.truetype_cmap().is_none());
    }

    #[test]
    fn test_has_truetype_cmap_no_data() {
        let font = make_font(|f| {
            f.is_truetype_font = false;
        });
        assert!(!font.has_truetype_cmap());
    }

    #[test]
    fn test_set_truetype_cmap_to_none() {
        let mut font = make_font(|_| {});
        font.set_truetype_cmap(None);
        assert!(font.truetype_cmap().is_none());
    }

    #[test]
    fn test_cid_to_gid_explicit_empty() {
        let map = CIDToGIDMap::Explicit(vec![]);
        assert_eq!(map.get_gid(0), 0);
        assert_eq!(map.get_gid(100), 100);
    }

    #[test]
    fn test_cid_to_gid_explicit_boundary() {
        let map = CIDToGIDMap::Explicit(vec![99, 88]);
        assert_eq!(map.get_gid(0), 99);
        assert_eq!(map.get_gid(1), 88);
        assert_eq!(map.get_gid(2), 2);
    }

    #[test]
    fn test_cid_to_gid_identity_max() {
        let map = CIDToGIDMap::Identity;
        assert_eq!(map.get_gid(u16::MAX), u16::MAX);
    }

    #[test]
    fn test_char_to_unicode_type0_identity_agl_fallback() {
        let font = make_font(|f| {
            f.base_font = "SubsetFont+F3".to_string();
            f.subtype = "Type0".to_string();
            f.encoding = Encoding::Identity;
            f.cid_to_gid_map = Some(CIDToGIDMap::Identity);
        });
        assert_eq!(font.char_to_unicode(0x41), Some("A".to_string()));
    }

    #[test]
    fn test_char_to_unicode_type0_rksj() {
        let font = make_font(|f| {
            f.subtype = "Type0".to_string();
            f.encoding = Encoding::Standard("90ms-RKSJ-H".to_string());
        });
        assert_eq!(font.char_to_unicode(0x41), Some("A".to_string()));
    }

    #[test]
    fn test_char_to_unicode_type0_identity_v() {
        let font = make_font(|f| {
            f.subtype = "Type0".to_string();
            f.encoding = Encoding::Standard("Identity-V".to_string());
        });
        assert_eq!(font.char_to_unicode(0x42), Some("B".to_string()));
    }

    #[test]
    fn test_char_to_unicode_unknown_standard_encoding() {
        let font = make_font(|f| {
            f.encoding = Encoding::Standard("SomeRandomEncoding".to_string());
        });
        assert_eq!(font.char_to_unicode(0x41), Some("A".to_string()));
        assert_eq!(font.char_to_unicode(0x80), None);
    }

    #[test]
    fn test_encoding_identity_clone() {
        let enc = Encoding::Identity;
        let enc2 = enc.clone();
        assert!(matches!(enc2, Encoding::Identity));
    }

    #[test]
    fn test_encoding_custom_clone() {
        let mut map = HashMap::new();
        map.insert(1u8, 'X');
        let enc = Encoding::Custom(map);
        let enc2 = enc.clone();
        match enc2 {
            Encoding::Custom(m) => assert_eq!(m.get(&1), Some(&'X')),
            _ => panic!("Wrong encoding type"),
        }
    }

    #[test]
    fn test_encoding_debug() {
        let enc = Encoding::Standard("WinAnsiEncoding".to_string());
        let debug = format!("{:?}", enc);
        assert!(debug.contains("WinAnsiEncoding"));
    }

    #[test]
    fn test_cidsysteminfo_clone() {
        let info = CIDSystemInfo {
            registry: "Adobe".to_string(),
            ordering: "Japan1".to_string(),
            supplement: 6,
        };
        let info2 = info.clone();
        assert_eq!(info2.registry, "Adobe");
        assert_eq!(info2.ordering, "Japan1");
        assert_eq!(info2.supplement, 6);
    }

    #[test]
    fn test_cidsysteminfo_debug() {
        let info = CIDSystemInfo {
            registry: "Adobe".to_string(),
            ordering: "GB1".to_string(),
            supplement: 2,
        };
        let debug = format!("{:?}", info);
        assert!(debug.contains("Adobe"));
        assert!(debug.contains("GB1"));
    }

    #[test]
    fn test_cidtogidmap_clone() {
        let map = CIDToGIDMap::Explicit(vec![1, 2, 3]);
        let map2 = map.clone();
        assert_eq!(map2.get_gid(0), 1);
    }

    #[test]
    fn test_cidtogidmap_debug() {
        let map = CIDToGIDMap::Identity;
        let debug = format!("{:?}", map);
        assert!(debug.contains("Identity"));
    }

    #[test]
    fn test_char_to_unicode_type0_identity_large_cid() {
        let font = make_font(|f| {
            f.subtype = "Type0".to_string();
            f.encoding = Encoding::Identity;
            f.cid_to_gid_map = Some(CIDToGIDMap::Identity);
        });
        assert_eq!(font.char_to_unicode(0x10000), Some("\u{10000}".to_string()));
        // But a CID that maps to a control character should return FFFD ~keep
        assert_eq!(font.char_to_unicode(0x01), Some("\u{FFFD}".to_string()));
    }

    #[test]
    fn test_char_to_unicode_type0_predefined_cmap_japan1() {
        let font = make_font(|f| {
            f.subtype = "Type0".to_string();
            f.encoding = Encoding::Identity;
            f.cid_system_info = Some(CIDSystemInfo {
                registry: "Adobe".to_string(),
                ordering: "Japan1".to_string(),
                supplement: 4,
            });
        });
        assert_eq!(font.char_to_unicode(843), Some("\u{3042}".to_string()));
    }

    #[test]
    fn test_gid_to_standard_glyph_name_boundary_values() {
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0x20), Some("space"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0x7E), Some("asciitilde"));
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0x1F), None);
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0x7F), None);
        assert_eq!(FontInfo::gid_to_standard_glyph_name(0xFF), Some("ydieresis"));
    }

    #[test]
    fn test_glyph_name_to_unicode_math_symbols() {
        assert_eq!(glyph_name_to_unicode("infinity"), Some('∞'));
        assert_eq!(glyph_name_to_unicode("notequal"), Some('≠'));
        assert_eq!(glyph_name_to_unicode("lessequal"), Some('≤'));
        assert_eq!(glyph_name_to_unicode("greaterequal"), Some('≥'));
    }

    #[test]
    fn test_glyph_name_to_unicode_german_sharp_s() {
        assert_eq!(glyph_name_to_unicode("germandbls"), Some('ß'));
    }

    #[test]
    fn test_glyph_name_to_unicode_copyright_registered() {
        assert_eq!(glyph_name_to_unicode("copyright"), Some('©'));
        assert_eq!(glyph_name_to_unicode("registered"), Some('®'));
        assert_eq!(glyph_name_to_unicode("trademark"), Some('™'));
    }

    #[test]
    fn test_char_to_unicode_type0_identity_h_cjk_ordering() {
        let font = make_font(|f| {
            f.subtype = "Type0".to_string();
            f.encoding = Encoding::Standard("Identity-H".to_string());
            f.cid_system_info = Some(CIDSystemInfo {
                registry: "Adobe".to_string(),
                ordering: "Japan1".to_string(),
                supplement: 4,
            });
        });
        assert_eq!(font.char_to_unicode(843), Some("\u{3042}".to_string()));
    }

    #[test]
    fn test_char_to_unicode_type0_ucs2_encoding() {
        let font = make_font(|f| {
            f.subtype = "Type0".to_string();
            f.encoding = Encoding::Standard("UniJIS-UCS2-H".to_string());
            f.cid_system_info = Some(CIDSystemInfo {
                registry: "Adobe".to_string(),
                ordering: "Identity".to_string(),
                supplement: 0,
            });
        });
        assert_eq!(font.char_to_unicode(0x41), Some("A".to_string()));
    }

    #[test]
    fn test_standard_encoding_winansi_control_range() {
        assert_eq!(standard_encoding_lookup("WinAnsiEncoding", 0x00), None);
        assert_eq!(standard_encoding_lookup("WinAnsiEncoding", 0x01), None);
        assert_eq!(standard_encoding_lookup("WinAnsiEncoding", 0x1F), None);
    }

    #[test]
    fn test_standard_encoding_winansi_full_extended() {
        assert_eq!(
            standard_encoding_lookup("WinAnsiEncoding", 0x85),
            Some("\u{2026}".to_string())
        );
        assert_eq!(
            standard_encoding_lookup("WinAnsiEncoding", 0x99),
            Some("\u{2122}".to_string())
        );
        assert_eq!(
            standard_encoding_lookup("WinAnsiEncoding", 0xFF),
            Some("\u{00FF}".to_string())
        );
    }

    #[test]
    fn test_wrap_cff_in_opentype_header() {
        let cff = vec![1, 0, 4, 1, 0, 0, 0, 0];
        let otf = super::wrap_cff_in_opentype(&cff);

        assert_eq!(&otf[0..4], b"OTTO");
        assert_eq!(u16::from_be_bytes([otf[4], otf[5]]), 4);
        assert!(otf.windows(cff.len()).any(|w| w == &cff[..]));
    }

    #[test]
    fn test_wrap_cff_in_opentype_contains_required_tables() {
        let cff = vec![1, 0, 4, 1, 0, 0, 0, 0, 0, 0, 0, 0];
        let otf = super::wrap_cff_in_opentype(&cff);

        // Check all 4 required table tags exist in the table directory
        // Table directory starts at offset 12, each record is 16 bytes ~keep
        let mut found_tables = Vec::new();
        for i in 0..4 {
            let offset = 12 + i * 16;
            let tag = std::str::from_utf8(&otf[offset..offset + 4]).unwrap_or("????");
            found_tables.push(tag.to_string());
        }
        found_tables.sort();
        assert_eq!(found_tables, vec!["CFF ", "head", "hhea", "maxp"]);
    }

    #[test]
    fn test_wrap_cff_in_opentype_parseable() {
        let cff = vec![1, 0, 4, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let otf = super::wrap_cff_in_opentype(&cff);

        let result = skrifa::raw::FontRef::new(&otf);
        // May fail on CFF content but should not panic on table parsing
        // The fact that it doesn't panic is the test ~keep
        let _ = result;
    }

    #[test]
    fn test_standard_font_width_times_roman() {
        let font = FontInfo {
            base_font: "Times-Roman".to_string(),
            subtype: "Type1".to_string(),
            encoding: Encoding::Standard("WinAnsiEncoding".to_string()),
            to_unicode: None,
            font_weight: None,
            flags: None,
            stem_v: None,
            ascent: 0.95,
            descent: -0.35,
            embedded_font_data: None,
            truetype_cmap: std::sync::OnceLock::new(),
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font: false,
            cid_to_gid_map: None,
            cid_system_info: None,
            cid_font_type: None,
            widths: None,
            first_char: None,
            last_char: None,
            font_matrix_a: 0.001,
            default_width: 500.0,
            cid_widths: None,
            cid_default_width: 1000.0,
            has_explicit_dw: false,
            cff_gid_map: None,
            multi_char_map: HashMap::new(),
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names: std::collections::HashMap::new(),
            wmode: 0,
            cid_vertical_metrics: None,
            cid_default_vertical_metrics: VerticalMetrics::SPEC_DEFAULT,
            cjk_substitution: None,
        };

        assert_eq!(font.get_glyph_width(65), 722.0);
        assert_eq!(font.get_glyph_width(105), 278.0);
        assert_eq!(font.get_glyph_width(32), 250.0);
        assert_eq!(font.get_glyph_width(109), 778.0);
    }

    #[test]
    fn test_standard_font_width_courier_monospace() {
        let font = FontInfo {
            base_font: "Courier".to_string(),
            subtype: "Type1".to_string(),
            encoding: Encoding::Standard("WinAnsiEncoding".to_string()),
            to_unicode: None,
            font_weight: None,
            flags: None,
            stem_v: None,
            ascent: 0.95,
            descent: -0.35,
            embedded_font_data: None,
            truetype_cmap: std::sync::OnceLock::new(),
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font: false,
            cid_to_gid_map: None,
            cid_system_info: None,
            cid_font_type: None,
            widths: None,
            first_char: None,
            last_char: None,
            font_matrix_a: 0.001,
            default_width: 500.0,
            cid_widths: None,
            cid_default_width: 1000.0,
            has_explicit_dw: false,
            cff_gid_map: None,
            multi_char_map: HashMap::new(),
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names: std::collections::HashMap::new(),
            wmode: 0,
            cid_vertical_metrics: None,
            cid_default_vertical_metrics: VerticalMetrics::SPEC_DEFAULT,
            cjk_substitution: None,
        };

        assert_eq!(font.get_glyph_width(65), 600.0);
        assert_eq!(font.get_glyph_width(105), 600.0);
        assert_eq!(font.get_glyph_width(32), 600.0);
    }

    #[test]
    fn test_standard_font_width_not_applied_with_widths_array() {
        let font = FontInfo {
            base_font: "Times-Roman".to_string(),
            subtype: "Type1".to_string(),
            encoding: Encoding::Standard("WinAnsiEncoding".to_string()),
            to_unicode: None,
            font_weight: None,
            flags: None,
            stem_v: None,
            ascent: 0.95,
            descent: -0.35,
            embedded_font_data: None,
            truetype_cmap: std::sync::OnceLock::new(),
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font: false,
            cid_to_gid_map: None,
            cid_system_info: None,
            cid_font_type: None,
            widths: Some(vec![999.0]),
            first_char: Some(65),
            last_char: Some(65),
            font_matrix_a: 0.001,
            default_width: 500.0,
            cid_widths: None,
            cid_default_width: 1000.0,
            has_explicit_dw: false,
            cff_gid_map: None,
            multi_char_map: HashMap::new(),
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names: std::collections::HashMap::new(),
            wmode: 0,
            cid_vertical_metrics: None,
            cid_default_vertical_metrics: VerticalMetrics::SPEC_DEFAULT,
            cjk_substitution: None,
        };

        assert_eq!(font.get_glyph_width(65), 999.0);
    }

    #[test]
    fn test_standard_font_width_not_applied_to_unknown_font() {
        let font = FontInfo {
            base_font: "MyCustomFont".to_string(),
            subtype: "Type1".to_string(),
            encoding: Encoding::Standard("WinAnsiEncoding".to_string()),
            to_unicode: None,
            font_weight: None,
            flags: None,
            stem_v: None,
            ascent: 0.95,
            descent: -0.35,
            embedded_font_data: None,
            truetype_cmap: std::sync::OnceLock::new(),
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font: false,
            cid_to_gid_map: None,
            cid_system_info: None,
            cid_font_type: None,
            widths: None,
            first_char: None,
            last_char: None,
            font_matrix_a: 0.001,
            default_width: 500.0,
            cid_widths: None,
            cid_default_width: 1000.0,
            has_explicit_dw: false,
            cff_gid_map: None,
            multi_char_map: HashMap::new(),
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names: std::collections::HashMap::new(),
            wmode: 0,
            cid_vertical_metrics: None,
            cid_default_vertical_metrics: VerticalMetrics::SPEC_DEFAULT,
            cjk_substitution: None,
        };

        assert_eq!(font.get_glyph_width(65), 500.0);
    }

    /// Pins the standard-14 fallback path in `get_byte_to_width_table`:
    /// when `widths` is `None`, the table must be populated from
    /// `get_standard_font_width` (PDF spec Appendix D metrics), not
    /// from `default_width`. Also pins the fallback-within-the-fallback
    /// for byte codes that don't appear in the standard-14 table —
    /// those still use `default_width`.
    #[test]
    fn fallback_uses_standard_14_metrics_when_widths_absent() {
        let font = FontInfo {
            base_font: "Helvetica".to_string(),
            subtype: "Type1".to_string(),
            encoding: Encoding::Standard("WinAnsiEncoding".to_string()),
            to_unicode: None,
            font_weight: Some(400),
            flags: None,
            stem_v: None,
            ascent: 0.95,
            descent: -0.35,
            embedded_font_data: None,
            truetype_cmap: std::sync::OnceLock::new(),
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font: false,
            widths: None,
            first_char: None,
            last_char: None,
            font_matrix_a: 0.001,
            default_width: 1000.0,
            cid_to_gid_map: None,
            cid_system_info: None,
            cid_font_type: None,
            cid_widths: None,
            cid_default_width: 1000.0,
            has_explicit_dw: false,
            cff_gid_map: None,
            multi_char_map: HashMap::new(),
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names: std::collections::HashMap::new(),
            wmode: 0,
            cid_vertical_metrics: None,
            cid_default_vertical_metrics: VerticalMetrics::SPEC_DEFAULT,
            cjk_substitution: None,
        };

        let table = font.get_byte_to_width_table();

        assert_eq!(table[32], 278.0, "space");
        assert_eq!(table[48], 556.0, "digit '0'");
        assert_eq!(table[65], 667.0, "'A'");
        assert_eq!(table[87], 944.0, "'W'");

        assert_eq!(table[0], 1000.0, "NUL -> default_width fallback");
    }

    /// Build a minimal ToUnicode CMap stream that maps codes 0x0041–0x005A
    /// (hex 2-byte keys) to U+0041–U+005A (A–Z).
    fn make_tounicode_az() -> Vec<u8> {
        let stream = concat!(
            "/CIDInit /ProcSet findresource begin\n",
            "12 dict begin\n",
            "begincmap\n",
            "/CIDSystemInfo 3 dict dup begin\n",
            "  /Registry (Adobe) def\n",
            "  /Ordering (UCS) def\n",
            "  /Supplement 0 def\n",
            "end def\n",
            "/CMapName /Adobe-Identity-UCS def\n",
            "/CMapType 2 def\n",
            "1 begincodespacerange\n",
            "<0000> <FFFF>\n",
            "endcodespacerange\n",
            "26 beginbfchar\n",
            "<0041> <0041>\n",
            "<0042> <0042>\n",
            "<0043> <0043>\n",
            "<0044> <0044>\n",
            "<0045> <0045>\n",
            "<0046> <0046>\n",
            "<0047> <0047>\n",
            "<0048> <0048>\n",
            "<0049> <0049>\n",
            "<004A> <004A>\n",
            "<004B> <004B>\n",
            "<004C> <004C>\n",
            "<004D> <004D>\n",
            "<004E> <004E>\n",
            "<004F> <004F>\n",
            "<0050> <0050>\n",
            "<0051> <0051>\n",
            "<0052> <0052>\n",
            "<0053> <0053>\n",
            "<0054> <0054>\n",
            "<0055> <0055>\n",
            "<0056> <0056>\n",
            "<0057> <0057>\n",
            "<0058> <0058>\n",
            "<0059> <0059>\n",
            "<005A> <005A>\n",
            "endbfchar\n",
            "endcmap\n",
            "CMapName currentdict /CMap defineresource pop\n",
            "end\n",
            "end\n",
        );
        stream.as_bytes().to_vec()
    }

    /// Build a minimal ToUnicode CMap that maps code 0x0001 to U+0007 (BEL).
    fn make_tounicode_bel() -> Vec<u8> {
        let stream = concat!(
            "/CIDInit /ProcSet findresource begin\n",
            "12 dict begin\n",
            "begincmap\n",
            "/CIDSystemInfo 3 dict dup begin\n",
            "  /Registry (Adobe) def\n",
            "  /Ordering (UCS) def\n",
            "  /Supplement 0 def\n",
            "end def\n",
            "/CMapName /Test-BEL def\n",
            "/CMapType 2 def\n",
            "1 begincodespacerange\n",
            "<0000> <FFFF>\n",
            "endcodespacerange\n",
            "1 beginbfchar\n",
            "<0001> <0007>\n",
            "endbfchar\n",
            "endcmap\n",
            "CMapName currentdict /CMap defineresource pop\n",
            "end\n",
            "end\n",
        );
        stream.as_bytes().to_vec()
    }

    /// Construct a minimal Type0 FontInfo with the given ToUnicode stream and CIDSystemInfo.
    fn make_type0_font(
        to_unicode_stream: Option<Vec<u8>>,
        encoding_name: &str,
        cid_system_info: Option<CIDSystemInfo>,
    ) -> FontInfo {
        FontInfo {
            base_font: "TestType0Font".to_string(),
            subtype: "Type0".to_string(),
            // Mirror the real parser (`parse_encoding`): a `/Identity-H` or
            // `/Identity-V` encoding name resolves to `Encoding::Identity`, not
            // `Encoding::Standard("Identity-H")` — production never produces the
            // latter for an Identity name, so tests must not either. ~keep
            encoding: match encoding_name {
                "Identity-H" | "Identity-V" => Encoding::Identity,
                name => Encoding::Standard(name.to_string()),
            },
            to_unicode: to_unicode_stream.map(LazyCMap::new),
            font_weight: None,
            flags: None,
            stem_v: None,
            ascent: 0.95,
            descent: -0.35,
            embedded_font_data: None,
            truetype_cmap: std::sync::OnceLock::new(),
            embedded_glyph_names: std::sync::OnceLock::new(),
            is_truetype_font: false,
            widths: None,
            first_char: None,
            last_char: None,
            font_matrix_a: 0.001,
            default_width: 1000.0,
            cid_to_gid_map: None,
            cid_system_info,
            cid_font_type: None,
            cid_widths: None,
            cid_default_width: 1000.0,
            has_explicit_dw: false,
            cff_gid_map: None,
            multi_char_map: HashMap::new(),
            byte_to_char_table: std::sync::OnceLock::new(),
            type0_unicode_memo: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            byte_to_width_table: std::sync::OnceLock::new(),
            weight_memo: std::sync::OnceLock::new(),
            italic_memo: std::sync::OnceLock::new(),
            std14_memo: std::sync::OnceLock::new(),
            diff_glyph_names: std::collections::HashMap::new(),
            wmode: 0,
            cid_vertical_metrics: None,
            cid_default_vertical_metrics: VerticalMetrics::SPEC_DEFAULT,
            cjk_substitution: None,
        }
    }

    /// Fix A — ToUnicode present but code not covered → U+FFFD (no Priority-3 fallback).
    ///
    /// A Type0 font with Adobe-GB1 ordering, a *non-Identity* predefined-CMap
    /// encoding (`UniGB-UCS2-H` → `Encoding::Standard`), and a ToUnicode CMap
    /// covering only A–Z. The Fix-A guard is deliberately scoped to
    /// non-Identity Type0 fonts (Identity fonts map CID→Unicode directly
    /// have a valid CMap-miss fallback), so the encoding here must be a real
    /// predefined CMap — not Identity-H — for this guard to apply in
    /// production. Querying code 0x0061 (not in the ToUnicode CMap) must
    /// return U+FFFD, NOT the CJK character the Priority-3 predefined CMap
    /// lookup would otherwise produce.
    #[test]
    fn test_fix_a_tounicode_present_miss_returns_fffd_not_cjk() {
        let cid_system_info = Some(CIDSystemInfo {
            registry: "Adobe".to_string(),
            ordering: "GB1".to_string(),
            supplement: 2,
        });
        let font = make_type0_font(Some(make_tounicode_az()), "UniGB-UCS2-H", cid_system_info);

        let result = font.char_to_unicode(0x0061);
        assert_eq!(
            result,
            Some("\u{FFFD}".to_string()),
            "Type0 font with ToUnicode present but missing code 0x61 must return U+FFFD, \
             not fall through to predefined CMap"
        );

        assert_eq!(font.char_to_unicode(0x0041), Some("A".to_string()));
        assert_eq!(font.char_to_unicode(0x005A), Some("Z".to_string()));
    }

    /// Fix A — ToUnicode absent, Priority-3 predefined CMap is triggered.
    ///
    /// A Type0 font with Adobe-Japan1 ordering and NO ToUnicode CMap.
    /// Querying CID 843 must return U+3042 (あ) via the predefined CMap.
    ///
    /// `Identity-H` resolves to `Encoding::Identity` (as in production);
    /// combined with a non-Identity CIDSystemInfo ordering (Japan1) and no
    /// ToUnicode CMap, the lookup routes through the predefined-CMap path
    /// (`lookup_predefined_cmap`) rather than treating the CID as a raw
    /// Unicode code point.
    #[test]
    fn test_fix_a_no_tounicode_priority3_triggered() {
        let cid_system_info = Some(CIDSystemInfo {
            registry: "Adobe".to_string(),
            ordering: "Japan1".to_string(),
            supplement: 4,
        });
        let font = make_type0_font(None, "Identity-H", cid_system_info);

        let result = font.char_to_unicode(843);
        assert_eq!(
            result,
            Some("\u{3042}".to_string()),
            "Type0 font without ToUnicode must use predefined CMap for CID 843 → U+3042"
        );
    }

    /// Fix C — OOB CID guard: CID well beyond the Adobe-GB1 maximum → None.
    ///
    /// lookup_predefined_cmap with an OOB CID must return None without panicking.
    #[test]
    fn test_fix_c_oob_cid_returns_none() {
        let cid_system_info = Some(CIDSystemInfo {
            registry: "Adobe".to_string(),
            ordering: "GB1".to_string(),
            supplement: 2,
        });
        // CID 99_999 is far beyond CID_MAX_GB1 (30_283).
        // The function takes u16, so we use the max u16 value (65535) which still
        // exceeds CID_MAX_GB1. ~keep
        let result = lookup_predefined_cmap("UniGB-UCS2-H", &cid_system_info, 65535);
        assert_eq!(result, None, "OOB CID (65535 > CID_MAX_GB1 30283) must return None");

        let cid_japan = Some(CIDSystemInfo {
            registry: "Adobe".to_string(),
            ordering: "Japan1".to_string(),
            supplement: 4,
        });
        let result_j = lookup_predefined_cmap("UniJIS-UCS2-H", &cid_japan, 65535);
        assert_eq!(
            result_j, None,
            "OOB CID (65535 > CID_MAX_JAPAN1 23059) must return None"
        );
    }

    /// Fix B — C0 control character filter: ToUnicode mapping to U+0007 (BEL) → U+FFFD.
    ///
    /// A ToUnicode CMap that explicitly maps code 0x0001 to U+0007 (BEL).
    /// The function must return U+FFFD, not the BEL character.
    #[test]
    fn test_fix_b_control_char_filter_returns_fffd() {
        let font = make_type0_font(Some(make_tounicode_bel()), "Identity-H", None);

        let result = font.char_to_unicode(0x0001);
        assert_eq!(
            result,
            Some("\u{FFFD}".to_string()),
            "Code mapping to U+0007 (BEL) must be filtered to U+FFFD by Fix B"
        );
    }

    /// A ToUnicode CMap that maps only code 0x0041 → U+005A ('Z'); every other
    /// code is absent.
    fn make_tounicode_single_z() -> Vec<u8> {
        concat!(
            "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n",
            "/CIDSystemInfo 3 dict dup begin\n",
            "  /Registry (Adobe) def\n  /Ordering (UCS) def\n  /Supplement 0 def\nend def\n",
            "/CMapName /Test-Z def\n/CMapType 2 def\n",
            "1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
            "1 beginbfchar\n<0041> <005A>\nendbfchar\n",
            "endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n",
        )
        .as_bytes()
        .to_vec()
    }

    /// A structurally-valid `/ToUnicode` CMap with zero `bfchar`/`bfrange` entries:
    /// present but maps nothing. Must count as *absent*.
    fn make_tounicode_empty() -> Vec<u8> {
        concat!(
            "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n",
            "/CIDSystemInfo 3 dict dup begin\n",
            "  /Registry (Adobe) def\n  /Ordering (UCS) def\n  /Supplement 0 def\nend def\n",
            "/CMapName /Test-Empty def\n/CMapType 2 def\n",
            "1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
            "endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n",
        )
        .as_bytes()
        .to_vec()
    }

    /// With a present-but-incomplete `/ToUnicode` on an Identity-H Type0 font, a
    /// drawn CID absent from it has no Unicode anywhere in the file, so it must
    /// decode to U+FFFD rather than a numeric *guess* — the CID read as a code
    /// point, or the GID via the standard glyph-name table → AGL. Both guess
    /// paths are exercised: 0x0100 (CID-as-char) and 0x003A (gid 0x3A = "colon");
    /// `cid_to_gid_map` is set so the gid→glyph-name path is actually reachable.
    #[test]
    fn test_type0_tounicode_gap_returns_fffd_not_guess() {
        let mut font = make_type0_font(Some(make_tounicode_single_z()), "Identity-H", None);
        font.cid_to_gid_map = Some(CIDToGIDMap::Identity);

        assert_eq!(font.char_to_unicode(0x0041), Some("Z".to_string()));
        assert_eq!(
            font.char_to_unicode(0x0100),
            Some("\u{FFFD}".to_string()),
            "uncovered CID must not be guessed as CID-as-Unicode"
        );
        assert_eq!(
            font.char_to_unicode(0x003A),
            Some("\u{FFFD}".to_string()),
            "uncovered CID must not be guessed via gid→glyph-name→AGL"
        );
    }

    /// Without a `/ToUnicode`, the CID-as-Unicode heuristic still applies — many
    /// generators assign CID == Unicode — so this path must not regress to U+FFFD.
    #[test]
    fn test_type0_no_tounicode_keeps_cid_as_unicode() {
        let mut font = make_type0_font(None, "Identity-H", None);
        font.cid_to_gid_map = Some(CIDToGIDMap::Identity);
        assert_eq!(font.char_to_unicode(0x0100), Some("\u{0100}".to_string()));
    }

    /// For an Identity-ordered font with a present-but-incomplete `/ToUnicode`, an
    /// uncovered CID decodes to U+FFFD (honest gap) rather than the CID-as-Unicode guess —
    /// except whitespace (0x20 → space), which is retained so word boundaries survive.
    #[test]
    fn test_type0_identity_uncovered_cid_is_fffd_keeps_space() {
        let csi = CIDSystemInfo {
            registry: "Adobe".to_string(),
            ordering: "Identity".to_string(),
            supplement: 0,
        };
        let mut font = make_type0_font(Some(make_tounicode_single_z()), "Identity-H", Some(csi));
        font.cid_to_gid_map = Some(CIDToGIDMap::Identity);

        assert_eq!(font.char_to_unicode(0x0041), Some("Z".to_string()), "ToUnicode hit");
        assert_eq!(font.char_to_unicode(0x0020), Some(" ".to_string()), "space retained");
        assert_eq!(
            font.char_to_unicode(0x0043),
            Some("\u{FFFD}".to_string()),
            "uncovered non-space Identity CID must be U+FFFD"
        );
    }

    /// A present-but-*empty* `/ToUnicode` (0 bfchar/bfrange) maps nothing, so it must count
    /// as absent and an Identity-ordered font must recover its text via CID-as-Unicode. The
    /// `CIDToGIDMap` here remaps each letter to a low *punctuation* GID, so the GID→standard-
    /// glyph-name→AGL guess (if it ran) would yield `J)'(i#`; CID-as-Unicode must win instead.
    /// This is the faithful subset case the `CIDToGIDMap::Identity` variant can't reproduce.
    #[test]
    fn test_type0_identity_empty_tounicode_keeps_cid_as_unicode() {
        let csi = CIDSystemInfo {
            registry: "Adobe".to_string(),
            ordering: "Identity".to_string(),
            supplement: 1,
        };
        let mut font = make_type0_font(Some(make_tounicode_empty()), "Identity-H", Some(csi));

        let letters = [
            (0x004A, "J"),
            (0x0075, "u"),
            (0x0073, "s"),
            (0x0074, "t"),
            (0x0069, "i"),
            (0x006E, "n"),
        ];
        let mut gid_map = vec![0u16; 0x80];
        for (i, (cid, _)) in letters.iter().enumerate() {
            gid_map[*cid as usize] = 0x21 + i as u16;
        }
        font.cid_to_gid_map = Some(CIDToGIDMap::Explicit(gid_map));

        for (cid, ch) in letters {
            assert_eq!(
                font.char_to_unicode(cid),
                Some(ch.to_string()),
                "empty /ToUnicode + Identity ordering must use CID-as-Unicode for 0x{cid:04X}, \
                 not the GID→glyph-name guess"
            );
        }
    }

    /// `make_type0_font` must mirror the real `parse_encoding`
    /// mapping. A direct guard so a future revert of the helper is caught
    /// tightly (the Fix-A/B tests above only assert it *indirectly* via
    /// `char_to_unicode` outcomes).
    #[test]
    fn test_make_type0_font_encoding_matches_parser() {
        assert!(
            matches!(make_type0_font(None, "Identity-H", None).encoding, Encoding::Identity),
            "Identity-H must map to Encoding::Identity (production never yields Standard(\"Identity-H\"))"
        );
        assert!(
            matches!(make_type0_font(None, "Identity-V", None).encoding, Encoding::Identity),
            "Identity-V must map to Encoding::Identity"
        );
        match make_type0_font(None, "WinAnsiEncoding", None).encoding {
            Encoding::Standard(ref n) => assert_eq!(n, "WinAnsiEncoding"),
            other => panic!("non-Identity name must stay Encoding::Standard, got {other:?}"),
        }
        match make_type0_font(None, "UniGB-UCS2-H", None).encoding {
            Encoding::Standard(ref n) => assert_eq!(n, "UniGB-UCS2-H"),
            other => panic!("predefined CMap name must be Encoding::Standard, got {other:?}"),
        }
    }

    /// Type0/CID fonts read ascent/descent from the CIDFont descendant's FontDescriptor
    /// (§9.7.4 / Table 117), not from the Type0 wrapper (which has no top-level
    /// /FontDescriptor). Verify that `FontInfo::from_dict` on a Type0 font with a
    /// descendant FontDescriptor that carries Ascent=800 / Descent=-200 yields
    /// ascent ≈ 0.8 and descent ≈ -0.2 (both normalised from 1/1000-em to fraction-of-em).
    #[test]
    fn test_type0_ascent_descent_from_descendant_descriptor() {
        let mut desc: HashMap<String, Object> = HashMap::new();
        desc.insert("Type".to_string(), Object::Name("FontDescriptor".to_string()));
        desc.insert("Ascent".to_string(), Object::Integer(800));
        desc.insert("Descent".to_string(), Object::Integer(-200));

        let mut cidfont: HashMap<String, Object> = HashMap::new();
        cidfont.insert("Type".to_string(), Object::Name("Font".to_string()));
        cidfont.insert("Subtype".to_string(), Object::Name("CIDFontType0".to_string()));
        cidfont.insert("BaseFont".to_string(), Object::Name("TestCIDFont".to_string()));
        cidfont.insert("DW".to_string(), Object::Integer(1000));
        cidfont.insert(
            "CIDSystemInfo".to_string(),
            Object::Dictionary({
                let mut si = HashMap::new();
                si.insert("Registry".to_string(), Object::String(b"Adobe".to_vec()));
                si.insert("Ordering".to_string(), Object::String(b"Identity".to_vec()));
                si.insert("Supplement".to_string(), Object::Integer(0));
                si
            }),
        );
        cidfont.insert("FontDescriptor".to_string(), Object::Dictionary(desc));

        let mut type0: HashMap<String, Object> = HashMap::new();
        type0.insert("Type".to_string(), Object::Name("Font".to_string()));
        type0.insert("Subtype".to_string(), Object::Name("Type0".to_string()));
        type0.insert("BaseFont".to_string(), Object::Name("TestType0Font".to_string()));
        type0.insert("Encoding".to_string(), Object::Name("Identity-H".to_string()));
        type0.insert(
            "DescendantFonts".to_string(),
            Object::Array(vec![Object::Dictionary(cidfont)]),
        );

        let doc = minimal_pdf_doc();
        let font = FontInfo::from_dict(&Object::Dictionary(type0), &doc)
            .expect("Type0 font with inline descendant must parse");

        assert!(
            (font.ascent - 0.8).abs() < 1e-4,
            "Expected ascent ≈ 0.8 (800/1000), got {}",
            font.ascent
        );
        assert!(
            (font.descent - (-0.2)).abs() < 1e-4,
            "Expected descent ≈ -0.2 (-200/1000), got {}",
            font.descent
        );
    }

    /// A minimal in-memory PDF so `parse_encoding` (which takes `&PdfDocument`)
    /// can run in a unit test. The encoding dict and /Differences array below
    /// use only inline objects, so the document is never actually dereferenced.
    fn minimal_pdf_doc() -> crate::document::PdfDocument {
        let pdf = b"%PDF-1.4\n\
            1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
            2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n\
            3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>\nendobj\n\
            xref\n\
            0 4\n\
            0000000000 65535 f \n\
            0000000009 00000 n \n\
            0000000058 00000 n \n\
            0000000115 00000 n \n\
            trailer\n<< /Size 4 /Root 1 0 R >>\n\
            startxref\n197\n%%EOF\n";
        crate::document::PdfDocument::from_bytes(pdf.to_vec()).expect("minimal PDF should parse")
    }

    /// A synthetic CMMI-like encoding dict that parks `/period` at code 58.
    fn cmmi_like_encoding_obj() -> Object {
        let mut enc: HashMap<String, Object> = HashMap::new();
        enc.insert(
            "Differences".to_string(),
            Object::Array(vec![
                Object::Integer(44),
                Object::Name("arrowhookleft".to_string()),
                Object::Integer(58),
                Object::Name("period".to_string()),
            ]),
        );
        Object::Dictionary(enc)
    }

    /// Task 1 verify: the /Differences glyph name survives parse time.
    #[test]
    fn test_diff_glyph_names_retains_period_for_code_58() {
        let doc = minimal_pdf_doc();
        let (_enc, _multi, diff_names) = FontInfo::parse_encoding(&cmmi_like_encoding_obj(), &doc, None).unwrap();
        assert_eq!(diff_names.get(&58).map(String::as_str), Some("period"));
        assert_eq!(diff_names.get(&44).map(String::as_str), Some("arrowhookleft"));
    }

    /// Task 2 verify: the closed-set AGL punctuation helper.
    #[test]
    fn test_punctuation_unicode_for_glyph_name_closed_set() {
        assert_eq!(punctuation_unicode_for_glyph_name("period"), Some("."));
        assert_eq!(punctuation_unicode_for_glyph_name("comma"), Some(","));
        assert_eq!(punctuation_unicode_for_glyph_name("hyphen"), Some("-"));
        assert_eq!(punctuation_unicode_for_glyph_name("minus"), Some("\u{2212}"));
        assert_eq!(punctuation_unicode_for_glyph_name("colon"), None);
        assert_eq!(punctuation_unicode_for_glyph_name("logicalnot"), None);
        assert_eq!(punctuation_unicode_for_glyph_name("A"), None);
    }

    /// Task 2 verify: the non-sensible-symbol predicate.
    #[test]
    fn test_is_non_sensible_symbol() {
        assert!(is_non_sensible_symbol("\u{00AC}"));
        assert!(is_non_sensible_symbol("\u{2192}"));
        assert!(is_non_sensible_symbol("\u{2212}"));
        assert!(!is_non_sensible_symbol("."));
        assert!(!is_non_sensible_symbol(","));
        assert!(!is_non_sensible_symbol("-"));
        assert!(!is_non_sensible_symbol("5"));
        assert!(!is_non_sensible_symbol("A"));
        assert!(!is_non_sensible_symbol(""));
        assert!(!is_non_sensible_symbol("ff"));
    }

    /// Build a CMMI-like simple font with the given ToUnicode CMap bytes and a
    /// `/Differences 58 /period` side map (and matching Custom encoding entry).
    fn cmmi_like_font(to_unicode: Option<&[u8]>, custom_char_for_58: char) -> FontInfo {
        let mut diff_glyph_names: HashMap<u8, String> = HashMap::new();
        diff_glyph_names.insert(58, "period".to_string());
        let mut custom_map: HashMap<u8, char> = HashMap::new();
        custom_map.insert(58, custom_char_for_58);
        make_font(|f| {
            f.base_font = "SQLQIW+CMMI10".to_string();
            f.subtype = "Type1".to_string();
            f.flags = Some(4);
            f.encoding = Encoding::Custom(custom_map);
            f.to_unicode = to_unicode.map(|b| LazyCMap::new(b.to_vec()));
            f.diff_glyph_names = diff_glyph_names;
        })
    }

    /// Task 3 verify (Interception A): a non-sensible ToUnicode hit (U+00AC) for
    /// a `/period`-named code is recovered to `.`.
    #[test]
    fn test_interception_a_tounicode_non_sensible_symbol_recovered() {
        let cmap = b"beginbfchar\n<003A> <00AC>\nendbfchar";
        let font = cmmi_like_font(Some(cmap), '.');
        assert_eq!(font.char_to_unicode(0x3A), Some(".".to_string()));
    }

    /// Task 4 verify (Interception B): no ToUnicode, Custom encoding resolves 58
    /// to a wrong symbol, but the /Differences /period name overrides to `.`.
    #[test]
    fn test_interception_b_custom_encoding_punctuation_override() {
        let font = cmmi_like_font(None, '\u{00AC}');
        assert_eq!(font.char_to_unicode(0x3A), Some(".".to_string()));
    }

    /// Task 5 regression guard: correctly-mapped fonts and genuine symbols are
    /// untouched by the punctuation-recovery interceptions.
    #[test]
    fn test_punctuation_recovery_regression_guard() {
        // (a) A correctly-mapped period via ToUnicode (0x2E → U+002E) with no
        //     special glyph name stays `.` — the hit is already sensible so
        //     Interception A never fires. ~keep
        let cmap_ok = b"beginbfchar\n<002E> <002E>\nendbfchar";
        let font_ok = make_font(|f| {
            f.to_unicode = Some(LazyCMap::new(cmap_ok.to_vec()));
        });
        assert_eq!(font_ok.char_to_unicode(0x2E), Some(".".to_string()));

        // (b) A genuine `logicalnot` glyph (¬) must stay ¬: its /Differences
        //     name is NOT in the punctuation closed set, so neither
        //     interception fires even though the resolved char is a symbol. ~keep
        let cmap_not = b"beginbfchar\n<0021> <00AC>\nendbfchar";
        let mut diff_glyph_names: HashMap<u8, String> = HashMap::new();
        diff_glyph_names.insert(0x21, "logicalnot".to_string());
        let font_not = make_font(|f| {
            f.base_font = "NSCCOE+txexs".to_string();
            f.flags = Some(4);
            f.to_unicode = Some(LazyCMap::new(cmap_not.to_vec()));
            f.diff_glyph_names = diff_glyph_names;
        });
        assert_eq!(font_not.char_to_unicode(0x21), Some("\u{00AC}".to_string()));
    }

    /// Build a synthetic single-page PDF whose object 4 is a Type0 font with
    /// the given base name, /Encoding name, descendant /CIDSystemInfo
    /// Ordering, and (optionally) an extra raw entry spliced into the
    /// descendant's FontDescriptor (e.g. `/FontFile3 99 0 R` pointing at a
    /// non-existent object to model a present-but-unextractable program).
    fn build_predefined_cidfont_pdf(
        base_font: &str,
        encoding_name: &str,
        ordering: &str,
        descriptor_extra: &str,
    ) -> Vec<u8> {
        let mut pdf = Vec::new();
        pdf.extend_from_slice(b"%PDF-1.4\n");

        let o1 = pdf.len();
        pdf.extend_from_slice(b"1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n");
        let o2 = pdf.len();
        pdf.extend_from_slice(b"2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n");
        let o3 = pdf.len();
        pdf.extend_from_slice(
            b"3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
              /Resources << /Font << /F1 4 0 R >> >> >> endobj\n",
        );
        let o4 = pdf.len();
        pdf.extend_from_slice(
            format!(
                "4 0 obj << /Type /Font /Subtype /Type0 /BaseFont /{base_font} \
                 /Encoding /{encoding_name} /DescendantFonts [5 0 R] >> endobj\n"
            )
            .as_bytes(),
        );
        let o5 = pdf.len();
        pdf.extend_from_slice(
            format!(
                "5 0 obj << /Type /Font /Subtype /CIDFontType0 /BaseFont /{base_font} \
                 /CIDSystemInfo << /Registry (Adobe) /Ordering ({ordering}) /Supplement 6 >> \
                 /FontDescriptor 6 0 R /DW 1000 >> endobj\n"
            )
            .as_bytes(),
        );
        let o6 = pdf.len();
        pdf.extend_from_slice(
            format!(
                "6 0 obj << /Type /FontDescriptor /FontName /{base_font} /Flags 6 \
                 /FontBBox [-170 -331 1024 903] /ItalicAngle 0 /Ascent 723 \
                 /Descent -241 /CapHeight 709 /StemV 69 {descriptor_extra} >> endobj\n"
            )
            .as_bytes(),
        );

        let xref = pdf.len();
        pdf.extend_from_slice(b"xref\n0 7\n0000000000 65535 f \n");
        for off in [o1, o2, o3, o4, o5, o6] {
            pdf.extend_from_slice(format!("{:010} 00000 n \n", off).as_bytes());
        }
        pdf.extend_from_slice(format!("trailer << /Size 7 /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n", xref).as_bytes());
        pdf
    }

    /// Parse object 4 of a [`build_predefined_cidfont_pdf`] document through
    /// the real `FontInfo::from_dict` path and return the resulting FontInfo.
    fn parse_predefined_cidfont(
        base_font: &str,
        encoding_name: &str,
        ordering: &str,
        descriptor_extra: &str,
    ) -> FontInfo {
        let pdf = build_predefined_cidfont_pdf(base_font, encoding_name, ordering, descriptor_extra);
        let doc = crate::document::PdfDocument::from_bytes(pdf).expect("synthetic PDF must parse");
        let font_obj = doc
            .load_object(crate::object::ObjectRef::new(4, 0))
            .expect("load Type0 font dict");
        FontInfo::from_dict(&font_obj, &doc).expect("FontInfo::from_dict")
    }

    /// Control: an Identity-H predefined name with no font program is flagged
    /// for substitution under the collection derived from the name.
    #[test]
    fn cjk_substitution_flags_identity_h_predefined_name() {
        let info = parse_predefined_cidfont("Ryumin-Light", "Identity-H", "Japan1", "");
        assert_eq!(
            info.cjk_substitution,
            Some(super::super::predefined_cidfont::CharacterCollection::AdobeJapan1)
        );
    }

    /// A descriptor that *declares* a font program (here a /FontFile3 whose
    /// target object doesn't exist, so extraction fails) must NOT be
    /// substituted: the document intended to embed outlines and the decode
    /// failure should surface as a warning, not be masked by a silent
    /// sans-serif substitution.
    #[test]
    fn cjk_substitution_declined_when_font_program_key_present_but_unextractable() {
        let info = parse_predefined_cidfont("Ryumin-Light", "Identity-H", "Japan1", "/FontFile3 99 0 R");
        assert!(info.embedded_font_data.is_none(), "extraction must have failed");
        assert_eq!(
            info.cjk_substitution, None,
            "substitution must not mask a failed embedded-font decode"
        );
    }

    /// Non-Identity predefined CMaps (90ms-RKSJ-H, GBK-EUC-H, …) carry raw
    /// legacy multi-byte codes, not CIDs. Until a charcode→CID CMap pass is
    /// wired, such fonts must not be substituted — interpreting a Shift-JIS
    /// code as an Adobe-Japan1 CID paints wrong glyphs with no diagnostic.
    #[test]
    fn cjk_substitution_requires_identity_cmap_encoding() {
        let info = parse_predefined_cidfont("Ryumin-Light-90ms-RKSJ-H", "90ms-RKSJ-H", "Japan1", "");
        assert_eq!(
            info.cjk_substitution, None,
            "non-Identity CMap codes are not CIDs; substitution must decline"
        );
    }

    /// When the descendant's /CIDSystemInfo names a known collection that
    /// disagrees with the one derived from the base-font name, the explicit
    /// CIDSystemInfo wins — it is authoritative for CID semantics per
    /// ISO 32000-1 §9.7.3.
    #[test]
    fn cjk_substitution_prefers_cid_system_info_ordering_over_name() {
        let info = parse_predefined_cidfont("Ryumin-Light", "Identity-H", "GB1", "");
        assert_eq!(
            info.cjk_substitution,
            Some(super::super::predefined_cidfont::CharacterCollection::AdobeGB1),
            "explicit /CIDSystemInfo Ordering must override the name-derived collection"
        );
    }

    /// An Identity (or unknown) Ordering carries no collection semantics; the
    /// name-derived collection remains the best available signal.
    #[test]
    fn cjk_substitution_keeps_name_collection_for_identity_ordering() {
        let info = parse_predefined_cidfont("Ryumin-Light", "Identity-H", "Identity", "");
        assert_eq!(
            info.cjk_substitution,
            Some(super::super::predefined_cidfont::CharacterCollection::AdobeJapan1)
        );
    }
}
