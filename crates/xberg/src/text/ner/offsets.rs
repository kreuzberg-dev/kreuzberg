//! Source-offset reconciliation for NER backends.
//!
//! Two problems live here, both of which caused PII to survive redaction:
//!
//! - **Windowing.** Every bundled NER model has a bounded input length. The
//!   GLiNER preprocessor silently truncates its input at
//!   `Parameters::max_length` words, so on a long document every entity past
//!   that point was never detected and the PII it covered was never redacted
//!   (xberg-io/xberg#262). [`split_into_windows`] cuts the source into
//!   overlapping windows that each fit the budget, and
//!   [`merge_windowed_entities`] folds the per-window detections back into
//!   source coordinates.
//! - **Occurrences.** A backend that reports a mention string without offsets
//!   (the LLM backend) must be resolved against the source. Resolving only the
//!   first hit leaves every later copy of the same mention in the output
//!   (xberg-io/xberg#200); [`entities_for_every_occurrence`] resolves all of
//!   them.
//!
//! Nothing here logs or formats the mention text itself — this module handles
//! PII and must not leak it into diagnostics.

use std::collections::HashMap;

use crate::types::entity::{Entity, EntityCategory};

/// Word-token budget for one GLiNER inference window.
///
/// Mirrors `xberg_gliner::Parameters::max_length`'s default. Callers should
/// pass the value their `Parameters` actually carry and fall back to this.
pub const GLINER_WINDOW_TOKENS: usize = 512;

/// Token overlap between adjacent windows.
///
/// A mention straddling a window edge is only detectable if some window
/// contains it whole; the overlap must therefore be comfortably wider than the
/// longest span the model can emit (`Parameters::max_width`, 12 words by
/// default).
pub const GLINER_WINDOW_OVERLAP_TOKENS: usize = 64;

/// One inference window and where it starts in the source text.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(alef, alef(skip))]
pub struct TextWindow {
    /// Byte offset of `text` within the source string.
    pub byte_offset: usize,
    /// The window's own text.
    pub text: String,
}

/// True for characters the GLiNER word splitter folds into a single token.
fn is_word_char(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

/// Byte spans of the word tokens in `text`.
///
/// Approximates the GLiNER splitter regex (`\w+(?:[-_]\w+)*|\S`): a run of word
/// characters is one token and any other non-whitespace character is a token on
/// its own. Hyphen-joined words count as three tokens here where GLiNER counts
/// one, which makes the window budget conservative — windows come out slightly
/// shorter than the model limit, never longer.
fn token_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut characters = text.char_indices().peekable();

    while let Some((index, character)) = characters.next() {
        if character.is_whitespace() {
            continue;
        }
        if !is_word_char(character) {
            spans.push((index, index + character.len_utf8()));
            continue;
        }
        let mut end = index + character.len_utf8();
        while let Some(&(next_index, next_character)) = characters.peek() {
            if !is_word_char(next_character) {
                break;
            }
            end = next_index + next_character.len_utf8();
            characters.next();
        }
        spans.push((index, end));
    }

    spans
}

/// Split `text` into overlapping windows of at most `window_tokens` word tokens.
///
/// Returns an empty vector when `text` holds no word tokens at all. Every
/// window starts and ends on a token boundary, so no mention is ever cut in
/// half by the split itself; `overlap_tokens` then guarantees that a mention
/// near a boundary is still contained whole in one of the two windows.
#[cfg_attr(alef, alef(skip))]
pub fn split_into_windows(text: &str, window_tokens: usize, overlap_tokens: usize) -> Vec<TextWindow> {
    let tokens = token_spans(text);
    if tokens.is_empty() {
        return Vec::new();
    }

    let window_tokens = window_tokens.max(1);
    let overlap_tokens = overlap_tokens.min(window_tokens - 1);
    let step = window_tokens - overlap_tokens;

    let mut windows = Vec::new();
    let mut first_token = 0usize;
    loop {
        let last_token = (first_token + window_tokens).min(tokens.len());
        let start_byte = tokens[first_token].0;
        let end_byte = tokens[last_token - 1].1;
        windows.push(TextWindow {
            byte_offset: start_byte,
            text: text[start_byte..end_byte].to_string(),
        });
        if last_token >= tokens.len() {
            break;
        }
        first_token += step;
    }

    windows
}

/// Fold per-window detections back into a single source-coordinate stream.
///
/// `window_offsets` and `per_window` are parallel: `per_window[i]` holds the
/// entities detected in the window starting at `window_offsets[i]`. Entities
/// are shifted into source coordinates, sorted, and deduplicated — the overlap
/// region between adjacent windows otherwise reports the same mention twice.
/// Where two same-category spans overlap the longer one wins; spans that cannot
/// be expressed as `u32` source offsets are dropped rather than reported wrong.
#[cfg_attr(alef, alef(skip))]
pub fn merge_windowed_entities(window_offsets: &[usize], per_window: Vec<Vec<Entity>>) -> Vec<Entity> {
    let mut all: Vec<Entity> = Vec::new();

    for (offset, entities) in window_offsets.iter().zip(per_window) {
        for mut entity in entities {
            let start = offset.saturating_add(entity.start as usize);
            let end = offset.saturating_add(entity.end as usize);
            let (Ok(start), Ok(end)) = (u32::try_from(start), u32::try_from(end)) else {
                continue;
            };
            entity.start = start;
            entity.end = end;
            all.push(entity);
        }
    }

    all.sort_by(|a, b| {
        a.start
            .cmp(&b.start)
            .then((b.end.saturating_sub(b.start)).cmp(&(a.end.saturating_sub(a.start))))
    });

    let mut furthest_end: HashMap<EntityCategory, u32> = HashMap::new();
    let mut kept: Vec<Entity> = Vec::with_capacity(all.len());
    for entity in all {
        if let Some(&previous_end) = furthest_end.get(&entity.category)
            && entity.start < previous_end
        {
            continue;
        }
        furthest_end.insert(entity.category.clone(), entity.end);
        kept.push(entity);
    }

    kept
}

/// Resolve `mention` into one [`Entity`] per occurrence in `text`.
///
/// Backends that report a mention without offsets must not stop at the first
/// hit: redacting only the first occurrence leaves every later copy of the same
/// PII in the output (xberg-io/xberg#200). Occurrences are non-overlapping and
/// returned in ascending byte order; an empty mention yields nothing.
#[cfg_attr(alef, alef(skip))]
pub fn entities_for_every_occurrence(
    text: &str,
    mention: &str,
    category: EntityCategory,
    confidence: Option<f32>,
) -> Vec<Entity> {
    if mention.is_empty() {
        return Vec::new();
    }

    let mut out = Vec::new();
    for (start, matched) in text.match_indices(mention) {
        let (Ok(start), Ok(end)) = (u32::try_from(start), u32::try_from(start + matched.len())) else {
            continue;
        };
        out.push(Entity {
            category: category.clone(),
            text: mention.to_string(),
            start,
            end,
            confidence,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person(start: u32, end: u32, text: &str) -> Entity {
        Entity {
            category: EntityCategory::Person,
            text: text.to_string(),
            start,
            end,
            confidence: None,
        }
    }

    #[test]
    fn token_spans_splits_words_and_punctuation() {
        assert_eq!(token_spans("Ada, Bob"), vec![(0, 3), (3, 4), (5, 8)]);
    }

    #[test]
    fn token_spans_is_empty_for_whitespace_only_text() {
        assert!(token_spans("  \n\t ").is_empty());
    }

    #[test]
    fn split_into_windows_returns_single_window_when_text_fits() {
        let windows = split_into_windows("Ada Lovelace works here", 512, 64);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].byte_offset, 0);
        assert_eq!(windows[0].text, "Ada Lovelace works here");
    }

    #[test]
    fn split_into_windows_covers_every_token_past_the_budget() {
        let text = (0..50).map(|index| format!("w{index}")).collect::<Vec<_>>().join(" ");
        let windows = split_into_windows(&text, 10, 2);

        assert!(windows.len() > 1, "long text must be windowed");
        assert_eq!(windows[0].byte_offset, 0);
        let last = windows.last().expect("at least one window");
        assert!(
            last.text.ends_with("w49"),
            "final window must reach the end of the source: {}",
            last.text
        );
        for window in &windows {
            assert_eq!(
                &text[window.byte_offset..window.byte_offset + window.text.len()],
                window.text,
                "window byte_offset must locate its own text in the source"
            );
        }
    }

    #[test]
    fn split_into_windows_overlaps_adjacent_windows() {
        let text = (0..30).map(|index| format!("w{index}")).collect::<Vec<_>>().join(" ");
        let windows = split_into_windows(&text, 10, 3);
        assert!(windows.len() >= 3);
        assert!(
            windows[1].byte_offset < windows[0].byte_offset + windows[0].text.len(),
            "window 1 must start before window 0 ends"
        );
    }

    #[test]
    fn merge_windowed_entities_shifts_into_source_coordinates() {
        let merged = merge_windowed_entities(&[0, 100], vec![vec![person(0, 3, "Ada")], vec![person(4, 7, "Bob")]]);
        assert_eq!(merged.len(), 2);
        assert_eq!((merged[0].start, merged[0].end), (0, 3));
        assert_eq!((merged[1].start, merged[1].end), (104, 107));
    }

    #[test]
    fn merge_windowed_entities_collapses_overlap_duplicates() {
        let merged = merge_windowed_entities(
            &[0, 90],
            vec![vec![person(100, 103, "Ada")], vec![person(10, 13, "Ada")]],
        );
        assert_eq!(merged.len(), 1, "same span from two windows must collapse: {merged:?}");
        assert_eq!((merged[0].start, merged[0].end), (100, 103));
    }

    #[test]
    fn entities_for_every_occurrence_finds_all_three_occurrences() {
        let text = "Ada paid Ada then Ada left";
        let found = entities_for_every_occurrence(text, "Ada", EntityCategory::Person, Some(0.9));
        assert_eq!(found.len(), 3);
        assert_eq!(
            found.iter().map(|e| (e.start, e.end)).collect::<Vec<_>>(),
            vec![(0, 3), (9, 12), (18, 21)]
        );
        for entity in &found {
            assert_eq!(&text[entity.start as usize..entity.end as usize], "Ada");
        }
    }

    #[test]
    fn entities_for_every_occurrence_returns_nothing_for_empty_mention() {
        assert!(entities_for_every_occurrence("Ada", "", EntityCategory::Person, None).is_empty());
    }
}
