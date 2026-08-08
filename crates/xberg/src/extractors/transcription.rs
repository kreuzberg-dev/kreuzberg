//! Built-in audio/video transcription extractor (speech-to-text).
//!
//! Only compiled when the `transcription` feature is enabled.
//! Registers for the audio and video MIME types declared in `core::mime`.
//!
//! The actual heavy lifting (model download + ORT inference) lives in
//! `crate::transcription`. This module is the thin "plugin" adapter that
//! the registry expects.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use crate::core::config::ExtractionConfig;
use crate::plugins::{InternalDocumentExtractor, Plugin};
use crate::transcription::decode::{PcmAudio, decode_audio_to_pcm};
use crate::transcription::engine::WhisperEngine;
use crate::transcription::model::{WhisperModelPaths, ensure_whisper_model};
use crate::transcription::tags::AudioTags;
use crate::types::internal::{ElementKind, InternalDocument, InternalElement};
use crate::types::metadata::{AudioMetadata, FormatMetadata};
use crate::{Result, XbergError};
use ahash::AHashMap;
use async_trait::async_trait;
use tokio::task;

/// Attribute key holding a segment's start time (milliseconds, as a decimal string).
const ATTR_START_MS: &str = "start_ms";
/// Attribute key holding a segment's end time (milliseconds, as a decimal string).
const ATTR_END_MS: &str = "end_ms";

/// Push transcript text onto `doc` as one or more `Paragraph` elements.
///
/// When `timestamps` is `false`, all segment text is joined into a single flat
/// paragraph (matching the pre-#306 behavior, since there is no per-segment
/// timing to preserve). When `true`, each non-empty `(start_ms, end_ms, text)`
/// segment becomes its own `Paragraph` element carrying `start_ms`/`end_ms`
/// attributes, so callers get segment boundaries and per-segment timestamps
/// without a new binding-visible type.
fn push_transcript_elements(doc: &mut InternalDocument, segments: &[(u32, u32, String)], timestamps: bool) {
    if timestamps {
        for (start_ms, end_ms, text) in segments {
            if text.is_empty() {
                continue;
            }
            let mut element = InternalElement::text(ElementKind::Paragraph, text.as_str(), 0);
            let mut attributes = AHashMap::default();
            attributes.insert(ATTR_START_MS.to_string(), start_ms.to_string());
            attributes.insert(ATTR_END_MS.to_string(), end_ms.to_string());
            element.attributes = Some(attributes);
            doc.push_element(element);
        }
        return;
    }

    let joined = segments
        .iter()
        .map(|(_, _, text)| text.as_str())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if !joined.is_empty() {
        doc.push_element(InternalElement::text(ElementKind::Paragraph, &joined, 0));
    }
}

/// Process-wide cache of loaded `WhisperEngine` instances, keyed by the
/// canonical model paths (encoder|tokenizer). Mirrors the pattern in
/// `crate::reranking::get_or_init_engine`.
static ENGINES: LazyLock<Mutex<HashMap<String, Arc<WhisperEngine>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Semaphore that limits the number of concurrent Whisper inference calls.
///
/// The budget matches `resolve_thread_budget` — the same value used by the
/// embedding and reranking semaphores so all ORT inference shares one
/// per-process concurrency bound.
static TRANSCRIPTION_SEMAPHORE: LazyLock<Arc<tokio::sync::Semaphore>> = LazyLock::new(|| {
    let budget = crate::core::config::concurrency::resolve_thread_budget(None);
    Arc::new(tokio::sync::Semaphore::new(budget))
});

/// Cache key for a loaded engine — stable across calls with identical model files.
fn engine_cache_key(paths: &WhisperModelPaths) -> String {
    format!("{}|{}", paths.encoder.display(), paths.tokenizer.display())
}

/// Return a cached `WhisperEngine` for `paths`, building and caching one on
/// the first call for each distinct model.
fn get_or_build_engine(paths: &WhisperModelPaths) -> Result<Arc<WhisperEngine>> {
    let key = engine_cache_key(paths);
    let mut map = ENGINES
        .lock()
        .map_err(|e| XbergError::transcription(format!("engine cache poisoned: {e}")))?;
    if let Some(engine) = map.get(&key) {
        return Ok(Arc::clone(engine));
    }
    let engine = WhisperEngine::load(paths)
        .map_err(|e| XbergError::transcription(format!("whisper engine load failed: {e}")))?;
    let arc = Arc::new(engine);
    map.insert(key, Arc::clone(&arc));
    Ok(arc)
}

/// Run `future` under a wall-clock deadline, bounding total async work.
///
/// `timeout_ms = None` disables the bound and simply awaits `future`. On
/// elapse, `future` is dropped (canceling any `.await` points inside it;
/// already-spawned `spawn_blocking` tasks keep running to completion in the
/// background but their result is discarded) and a
/// [`XbergError::Transcription`](crate::XbergError) is returned so callers get
/// a clear error instead of blocking forever.
async fn apply_timeout<T, Fut>(timeout_ms: Option<u64>, future: Fut) -> Result<T>
where
    Fut: std::future::Future<Output = Result<T>>,
{
    match timeout_ms {
        Some(ms) => tokio::time::timeout(std::time::Duration::from_millis(ms), future)
            .await
            .map_err(|_| {
                XbergError::transcription(format!(
                    "Transcription exceeded transcription.timeout_ms limit of {ms} ms. \
                     Increase `transcription.timeout_ms`, use a smaller Whisper model, or \
                     shorten the input."
                ))
            })?,
        None => future.await,
    }
}

/// Decode audio, resolve/load the Whisper model, and run inference.
///
/// This is the portion of transcription that [`TranscriptionExtractor::extract_content`]
/// bounds with [`TranscriptionConfig::timeout_ms`](crate::core::config::transcription::TranscriptionConfig::timeout_ms)
/// via [`apply_timeout`]. Split out as a free function (rather than inlined) so the
/// timeout wrapper composes cleanly around it.
async fn run_transcription_pipeline(
    content: &[u8],
    mime_type: &str,
    tcfg: &crate::core::config::transcription::TranscriptionConfig,
) -> Result<InternalDocument> {
    let bytes_owned = content.to_vec();
    let max_bytes_for_decode = tcfg.max_bytes;
    let (pcm, tags): (PcmAudio, crate::transcription::tags::AudioTags) = task::spawn_blocking(move || {
        let pcm = decode_audio_to_pcm(&bytes_owned, max_bytes_for_decode)?;
        let tags = crate::transcription::tags::read_audio_tags(&bytes_owned);
        Ok::<_, XbergError>((pcm, tags))
    })
    .await
    .map_err(|e| XbergError::transcription_with_source("Decoder task panicked", e))??;

    if let Some(max_dur) = tcfg.max_duration_ms
        && pcm.duration_ms > max_dur
    {
        return Err(XbergError::transcription(format!(
            "Decoded audio duration {} ms exceeds transcription.max_duration_ms limit of {}",
            pcm.duration_ms, max_dur
        )));
    }

    let paths = {
        let model = tcfg.model;
        let cache_dir = tcfg.model_cache_dir.clone();
        let allow_network = tcfg.allow_network;
        let verify_hash = tcfg.verify_hash;
        task::spawn_blocking(move || ensure_whisper_model(model, cache_dir.as_deref(), allow_network, verify_hash))
            .await
            .map_err(|e| XbergError::transcription(format!("model resolution task panicked: {e}")))?
            .map_err(|e| XbergError::transcription(format!("whisper model resolution failed: {e}")))?
    };

    let engine = get_or_build_engine(&paths)?;

    let _permit = TRANSCRIPTION_SEMAPHORE
        .acquire()
        .await
        .map_err(|e| XbergError::transcription(format!("semaphore closed: {e}")))?;

    let pcm_clone = pcm.clone();
    let lang_clone = tcfg.language.clone();
    let timestamps = tcfg.timestamps;
    let engine_for_task = Arc::clone(&engine);

    let segments = task::spawn_blocking(move || {
        engine_for_task.transcribe_segments(&pcm_clone, lang_clone.as_deref(), timestamps)
    })
    .await
    .map_err(|e| XbergError::transcription(format!("whisper task panicked: {e}")))?
    .map_err(|e| XbergError::transcription(format!("whisper inference failed: {e}")))?;

    let mut doc = build_audio_document(tags, &pcm, mime_type);
    push_transcript_elements(&mut doc, &segments, tcfg.timestamps);
    Ok(doc)
}

/// The transcription extractor.
///
/// Priority is the normal default (50). If a user registers a custom
/// higher-priority transcription backend via the plugin system, it will win.
#[cfg_attr(alef, alef(skip))]
pub struct TranscriptionExtractor;

impl Plugin for TranscriptionExtractor {
    fn name(&self) -> &str {
        "transcription"
    }

    fn version(&self) -> String {
        env!("CARGO_PKG_VERSION").to_string()
    }

    fn initialize(&self) -> Result<()> {
        Ok(())
    }

    fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl InternalDocumentExtractor for TranscriptionExtractor {
    async fn extract_content(
        &self,
        content: &[u8],
        mime_type: &str,
        config: &ExtractionConfig,
    ) -> Result<InternalDocument> {
        let tcfg = config.transcription.as_ref().filter(|c| c.enabled).ok_or_else(|| {
            XbergError::transcription(
                "Transcription requested for audio/video input, but no `transcription` \
                     config block was provided (or `enabled` is false). \
                     Add `transcription = { enabled = true, model = \"tiny\" }` (or equivalent) \
                     to your ExtractionConfig.",
            )
        })?;

        if let Some(max_b) = tcfg.max_bytes
            && content.len() as u64 > max_b
        {
            return Err(XbergError::transcription(format!(
                "Input size {} bytes exceeds transcription.max_bytes limit of {}",
                content.len(),
                max_b
            )));
        }

        apply_timeout(tcfg.timeout_ms, run_transcription_pipeline(content, mime_type, tcfg)).await
    }

    fn supported_mime_types(&self) -> &[&str] {
        // The `audio/mp3`, `audio/x-m4a`, `audio/x-wav` and `video/mpeg` entries are the
        // aliases core/mime.rs declares for the four canonical types beside them.
        // `validate_mime_type` accepts an alias verbatim and the registry looks extractors up
        // by exact string with no alias resolution, so an unclaimed alias is advertised as
        // supported and then fails as UnsupportedFormat (#229).
        &[
            "audio/mpeg",
            "audio/mp3",
            "audio/mp4",
            "audio/x-m4a",
            "audio/wav",
            "audio/x-wav",
            "audio/webm",
            "video/mp4",
            "video/mpeg",
            "video/webm",
        ]
    }

    fn priority(&self) -> i32 {
        50
    }
}

#[cfg(test)]
impl TranscriptionExtractor {
    fn extract_sync(&self, content: &[u8], mime_type: &str, config: &ExtractionConfig) -> Result<InternalDocument> {
        let tcfg = config.transcription.as_ref().filter(|c| c.enabled).ok_or_else(|| {
            XbergError::transcription(
                "Transcription requested for audio/video input, but no `transcription` \
                 config block was provided (or `enabled` is false). \
                 Add `transcription = { enabled = true, model = \"tiny\" }` (or equivalent) \
                 to your ExtractionConfig.",
            )
        })?;

        if let Some(max_b) = tcfg.max_bytes
            && content.len() as u64 > max_b
        {
            return Err(XbergError::transcription(format!(
                "Input size {} bytes exceeds transcription.max_bytes limit of {}",
                content.len(),
                max_b
            )));
        }

        let pcm = decode_audio_to_pcm(content, tcfg.max_bytes)?;
        let tags = crate::transcription::tags::read_audio_tags(content);

        if let Some(max_d) = tcfg.max_duration_ms
            && pcm.duration_ms > max_d
        {
            return Err(XbergError::transcription(format!(
                "Decoded audio duration {} ms exceeds transcription.max_duration_ms limit of {}",
                pcm.duration_ms, max_d
            )));
        }

        let paths = ensure_whisper_model(
            tcfg.model,
            tcfg.model_cache_dir.as_deref(),
            tcfg.allow_network,
            tcfg.verify_hash,
        )
        .map_err(|e| XbergError::transcription(format!("whisper model resolution failed: {e}")))?;

        let engine = get_or_build_engine(&paths)?;

        let segments = engine
            .transcribe_segments(&pcm, tcfg.language.as_deref(), tcfg.timestamps)
            .map_err(|e| XbergError::transcription(format!("whisper inference failed: {e}")))?;

        let mut doc = build_audio_document(tags, &pcm, mime_type);
        push_transcript_elements(&mut doc, &segments, tcfg.timestamps);
        Ok(doc)
    }
}

/// Construct an [`InternalDocument`] with metadata derived from audio tags and decoded PCM.
///
/// Populates the common [`Metadata`] fields (title, authors, created_at, language) from tag data
/// and attaches an [`AudioMetadata`] carrying codec/container/sample-rate/channel/bitrate info.
/// The caller pushes transcript text as a `Paragraph` element after Whisper inference.
fn build_audio_document(tags: AudioTags, pcm: &PcmAudio, mime_type: &str) -> InternalDocument {
    let audio_meta = AudioMetadata {
        duration_ms: tags.duration_ms.or(Some(pcm.duration_ms)),
        codec: tags.container.clone(),
        container: tags.container,
        sample_rate_hz: tags.sample_rate_hz.or(Some(pcm.sample_rate_hz)),
        channels: tags.channels.or(Some(pcm.channels)),
        bitrate: tags.bitrate,
    };

    let mut doc = InternalDocument::new("audio-transcript");
    doc.mime_type = mime_type.to_string();
    doc.metadata.title = tags.title;
    doc.metadata.authors = tags.artist.map(|a| vec![a]);
    doc.metadata.created_at = tags.year;
    doc.metadata.language = tags.language;
    doc.metadata.format = Some(FormatMetadata::Audio(audio_meta));
    doc
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::ExtractionConfig;
    use crate::core::config::transcription::{TranscriptionConfig, WhisperModel};

    #[test]
    fn test_transcription_extractor_metadata() {
        let ext = TranscriptionExtractor;
        assert_eq!(ext.name(), "transcription");
        assert!(ext.supported_mime_types().contains(&"audio/mpeg"));
        assert!(ext.supported_mime_types().contains(&"video/mp4"));
    }

    #[test]
    fn test_transcription_config_defaults_roundtrip() {
        let cfg = TranscriptionConfig {
            model: WhisperModel::Base,
            ..Default::default()
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: TranscriptionConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.model, WhisperModel::Base);
    }

    fn config_with_transcription(tcfg: TranscriptionConfig) -> ExtractionConfig {
        ExtractionConfig {
            transcription: Some(tcfg),
            ..Default::default()
        }
    }

    #[test]
    fn test_sync_no_config_returns_error() {
        let ext = TranscriptionExtractor;
        let cfg = ExtractionConfig::default();
        let result = ext.extract_sync(&[], "audio/mpeg", &cfg);
        assert!(result.is_err(), "expected error when no transcription config");
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("config") || msg.contains("disabled"), "unexpected: {msg}");
    }

    #[test]
    fn test_sync_disabled_config_returns_error() {
        let ext = TranscriptionExtractor;
        let tcfg = TranscriptionConfig {
            enabled: false,
            ..Default::default()
        };
        let cfg = config_with_transcription(tcfg);
        let result = ext.extract_sync(&[], "audio/mpeg", &cfg);
        assert!(result.is_err(), "expected error when transcription disabled");
    }

    #[test]
    fn test_sync_size_limit_enforced() {
        let ext = TranscriptionExtractor;
        let tcfg = TranscriptionConfig {
            max_bytes: Some(10),
            ..Default::default()
        };
        let cfg = config_with_transcription(tcfg);
        let oversized = vec![0u8; 11];
        let result = ext.extract_sync(&oversized, "audio/mpeg", &cfg);
        assert!(result.is_err(), "expected error when input exceeds max_bytes");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("exceed") || msg.contains("limit") || msg.contains("size"),
            "unexpected: {msg}"
        );
    }

    #[test]
    fn test_sync_duration_limit_enforced() {
        let wav_path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test_documents/audio/silence-1s.wav");
        let bytes = std::fs::read(&wav_path).unwrap_or_else(|e| panic!("missing audio fixture {wav_path:?}: {e}"));

        let ext = TranscriptionExtractor;
        let tcfg = TranscriptionConfig {
            max_duration_ms: Some(0),
            ..Default::default()
        };
        let cfg = config_with_transcription(tcfg);
        let result = ext.extract_sync(&bytes, "audio/wav", &cfg);
        assert!(result.is_err(), "expected error when decoded duration exceeds limit");
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("duration") || msg.contains("limit"), "unexpected: {msg}");
    }

    #[tokio::test]
    async fn test_async_no_config_returns_error() {
        let ext = TranscriptionExtractor;
        let cfg = ExtractionConfig::default();
        let result = ext.extract_content(&[], "audio/mpeg", &cfg).await;
        assert!(result.is_err(), "expected error when no transcription config (async)");
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("config") || msg.contains("disabled"), "unexpected: {msg}");
    }

    #[tokio::test]
    async fn test_async_size_limit_enforced() {
        let ext = TranscriptionExtractor;
        let tcfg = TranscriptionConfig {
            max_bytes: Some(10),
            ..Default::default()
        };
        let cfg = config_with_transcription(tcfg);
        let oversized = vec![0u8; 11];
        let result = ext.extract_content(&oversized, "audio/mpeg", &cfg).await;
        assert!(result.is_err(), "expected error when input exceeds max_bytes (async)");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("exceed") || msg.contains("limit") || msg.contains("size"),
            "unexpected: {msg}"
        );
    }

    /// Regression test for #278: `TranscriptionConfig::timeout_ms` had zero readers —
    /// every sibling field (`max_bytes`, `max_duration_ms`, `model_cache_dir`,
    /// `allow_network`, `verify_hash`) was enforced, but a transcription run had no
    /// wall-clock bound at all. `apply_timeout` is the mechanism `extract_content` now
    /// wraps the decode/model-resolution/inference pipeline in.
    #[tokio::test]
    async fn apply_timeout_returns_error_when_future_exceeds_timeout_ms() {
        let result: Result<()> = apply_timeout(Some(10), async {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            Ok(())
        })
        .await;
        assert!(result.is_err(), "expected timeout error, got {result:?}");
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("timeout_ms"), "unexpected message: {msg}");
    }

    #[tokio::test]
    async fn apply_timeout_passes_through_ok_when_future_finishes_in_time() {
        let result: Result<i32> = apply_timeout(Some(5_000), async { Ok(42) }).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn apply_timeout_passes_through_err_when_future_finishes_in_time() {
        let result: Result<i32> =
            apply_timeout(Some(5_000), async { Err(XbergError::transcription("inner failure")) }).await;
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("inner failure"), "unexpected message: {msg}");
    }

    #[tokio::test]
    async fn apply_timeout_with_none_never_times_out() {
        let result: Result<i32> = apply_timeout(None, async {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            Ok(7)
        })
        .await;
        assert_eq!(result.unwrap(), 7);
    }

    /// End-to-end wiring check: `extract_content` must actually read
    /// `transcription.timeout_ms` and apply it around the real pipeline, not just
    /// have `apply_timeout` exist unused. A `timeout_ms: Some(0)` deadline elapses
    /// before decode + model resolution can complete, so this exercises the real
    /// call path without requiring network access to resolve a Whisper model.
    #[tokio::test]
    async fn extract_content_enforces_timeout_ms() {
        let wav_path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test_documents/audio/silence-1s.wav");
        let bytes = std::fs::read(&wav_path).unwrap_or_else(|e| panic!("missing audio fixture {wav_path:?}: {e}"));

        let ext = TranscriptionExtractor;
        let tcfg = TranscriptionConfig {
            timeout_ms: Some(0),
            ..Default::default()
        };
        let cfg = config_with_transcription(tcfg);
        let result = ext.extract_content(&bytes, "audio/wav", &cfg).await;
        assert!(result.is_err(), "expected timeout error, got {result:?}");
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("timeout_ms"), "unexpected message: {msg}");
    }

    fn make_pcm(duration_ms: u64) -> PcmAudio {
        PcmAudio {
            samples: vec![],
            sample_rate_hz: 16_000,
            channels: 1,
            duration_ms,
        }
    }

    #[test]
    fn test_build_audio_document_populates_common_metadata() {
        let tags = AudioTags {
            title: Some("My Song".to_string()),
            artist: Some("Test Artist".to_string()),
            year: Some("2023".to_string()),
            language: Some("eng".to_string()),
            ..Default::default()
        };
        let pcm = make_pcm(90_000);
        let doc = build_audio_document(tags, &pcm, "audio/mpeg");

        assert_eq!(doc.metadata.title.as_deref(), Some("My Song"));
        assert_eq!(doc.metadata.authors.as_deref(), Some(&["Test Artist".to_string()][..]));
        assert_eq!(doc.metadata.created_at.as_deref(), Some("2023"));
        assert_eq!(doc.metadata.language.as_deref(), Some("eng"));
        assert_eq!(doc.mime_type, "audio/mpeg");
    }

    #[test]
    fn test_build_audio_document_populates_audio_format_metadata() {
        use crate::types::metadata::FormatMetadata;

        let tags = AudioTags {
            duration_ms: Some(30_000),
            sample_rate_hz: Some(44_100),
            channels: Some(2),
            bitrate: Some(320),
            container: Some("mp3".to_string()),
            ..Default::default()
        };
        let pcm = make_pcm(30_000);
        let doc = build_audio_document(tags, &pcm, "audio/mpeg");

        let Some(FormatMetadata::Audio(ref audio)) = doc.metadata.format else {
            panic!("expected FormatMetadata::Audio, got {:?}", doc.metadata.format);
        };
        assert_eq!(audio.duration_ms, Some(30_000));
        assert_eq!(audio.sample_rate_hz, Some(44_100));
        assert_eq!(audio.channels, Some(2));
        assert_eq!(audio.bitrate, Some(320));
        assert_eq!(audio.container.as_deref(), Some("mp3"));
    }

    #[test]
    fn test_build_audio_document_falls_back_to_pcm_properties() {
        use crate::types::metadata::FormatMetadata;

        let tags = AudioTags::default();
        let pcm = make_pcm(60_000);
        let doc = build_audio_document(tags, &pcm, "audio/wav");

        let Some(FormatMetadata::Audio(ref audio)) = doc.metadata.format else {
            panic!("expected FormatMetadata::Audio");
        };
        assert_eq!(
            audio.duration_ms,
            Some(60_000),
            "duration should fall back to PCM value"
        );
        assert_eq!(
            audio.sample_rate_hz,
            Some(16_000),
            "sample_rate should fall back to PCM value"
        );
        assert_eq!(audio.channels, Some(1), "channels should fall back to PCM value");
    }

    #[test]
    fn test_build_audio_document_empty_tags_no_common_metadata() {
        let tags = AudioTags::default();
        let pcm = make_pcm(0);
        let doc = build_audio_document(tags, &pcm, "audio/flac");

        assert!(doc.metadata.title.is_none(), "title should be absent for untagged file");
        assert!(
            doc.metadata.authors.is_none(),
            "authors should be absent for untagged file"
        );
        assert!(
            doc.metadata.created_at.is_none(),
            "created_at should be absent for untagged file"
        );
        assert!(
            doc.metadata.language.is_none(),
            "language should be absent for untagged file"
        );
    }
}
