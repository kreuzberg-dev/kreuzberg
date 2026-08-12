---
title: "Model Sources"
---

Xberg downloads ML models from the HuggingFace Hub on first use and caches them
under the platform cache directory (`~/.cache/xberg/` on Linux/macOS,
`%LOCALAPPDATA%/xberg/` on Windows), or under `XBERG_CACHE_DIR` when set.

Models that Xberg hosts itself live under the
[`xberg-io`](https://huggingface.co/xberg-io) organization. Everything else is
pulled from its upstream publisher. ONNX weights are SHA256-verified after
download where a checksum is pinned.

## Self-hosted (`xberg-io`)

Every model below is downloaded from a checksum-pinned `xberg-io` repository with weights unmodified from upstream. All are permissively licensed (Apache-2.0 or MIT).

| Capability | Repository | Notes |
| ---------- | ---------- | ----- |
| PaddleOCR — detection, classification, recognition (per-script + unified), text-line / document orientation | [`xberg-io/paddleocr-onnx-models`](https://huggingface.co/xberg-io/paddleocr-onnx-models) | PP-OCRv5 and PP-OCRv6 ONNX exports. |
| Table structure — SLANeXt (wired/wireless), SLANet+, table classifier | [`xberg-io/paddleocr-onnx-models`](https://huggingface.co/xberg-io/paddleocr-onnx-models) | `v2/table/*`, `v2/classifiers/*`. |
| Layout detection — RT-DETR, TATR, PP-DocLayout-V3 | [`xberg-io/layout-models`](https://huggingface.co/xberg-io/layout-models) | |
| PaddleOCR-VL 1.6 (candle vision-language OCR) | [`xberg-io/paddleocr-vl-1.6`](https://huggingface.co/xberg-io/paddleocr-vl-1.6) | Mirror of [`PaddlePaddle/PaddleOCR-VL-1.6`](https://huggingface.co/PaddlePaddle/PaddleOCR-VL-1.6) (Apache-2.0). SigLIP vision encoder + Ernie-4.5 text decoder; tasks `ocr`, `table`, `formula`, `chart`. |
| Named-entity recognition (GLiNER) | [`xberg-io/gliner-models`](https://huggingface.co/xberg-io/gliner-models) | xberg-managed span-mode ONNX exports and tokenizer files. Source model lineage is [`gliner-community`](https://huggingface.co/gliner-community). |
| Dense embeddings — gte-modernbert-base, arctic-embed-m-v2.0, qwen3-embedding-0.6b | [`xberg-io/embedding-models`](https://huggingface.co/xberg-io/embedding-models) | Mirrors of [`Alibaba-NLP/gte-modernbert-base`](https://huggingface.co/Alibaba-NLP/gte-modernbert-base) (Apache-2.0), [`Snowflake/snowflake-arctic-embed-m-v2.0`](https://huggingface.co/Snowflake/snowflake-arctic-embed-m-v2.0) (Apache-2.0), and [`Qwen/Qwen3-Embedding-0.6B`](https://huggingface.co/Qwen/Qwen3-Embedding-0.6B) (Apache-2.0). |
| Dense embeddings — potion-base-8M (`lightweight` preset, model2vec) | [`xberg-io/embedding-models`](https://huggingface.co/xberg-io/embedding-models) (`potion-base-8m/`) | Mirror of [`minishlab/potion-base-8M`](https://huggingface.co/minishlab/potion-base-8M) (MIT). Pure-Rust static backend — no ONNX Runtime. |
| Sparse embeddings — opensearch-v3-distill | [`xberg-io/sparse-embeddings`](https://huggingface.co/xberg-io/sparse-embeddings) | Mirror of [`opensearch-project/opensearch-neural-sparse-encoding-doc-v3-distill`](https://huggingface.co/opensearch-project/opensearch-neural-sparse-encoding-doc-v3-distill) (Apache-2.0). |
| Reranking — ettin-reranker-150m, qwen3-reranker-0.6b | [`xberg-io/reranker-models`](https://huggingface.co/xberg-io/reranker-models) | Mirrors of [`cross-encoder/ettin-reranker-150m-v1`](https://huggingface.co/cross-encoder/ettin-reranker-150m-v1) (Apache-2.0) and [`Qwen/Qwen3-Reranker-0.6B`](https://huggingface.co/Qwen/Qwen3-Reranker-0.6B) (Apache-2.0). `qwen3-reranker-0.6b` is a custom causal-LM ONNX export (generative reranker head). |
| Late interaction (ColBERT) — gte-moderncolbert | [`xberg-io/late-interaction-models`](https://huggingface.co/xberg-io/late-interaction-models) | Mirror of [`lightonai/GTE-ModernColBERT-v1`](https://huggingface.co/lightonai/GTE-ModernColBERT-v1) (Apache-2.0). |

For GLiNER, Xberg downloads only the exported artifacts listed in
`xberg-io/gliner-models`. If the repository is private or not publicly readable,
configure Hugging Face credentials supported by `hf-hub` before warming the
cache or running inference.

## Third-party

| Capability | Repositories |
| ---------- | ------------ |
| Embeddings | [`Xenova/all-MiniLM-L6-v2`](https://huggingface.co/Xenova/all-MiniLM-L6-v2), [`Xenova/bge-base-en-v1.5`](https://huggingface.co/Xenova/bge-base-en-v1.5), [`Xenova/bge-large-en-v1.5`](https://huggingface.co/Xenova/bge-large-en-v1.5), [`intfloat/multilingual-e5-base`](https://huggingface.co/intfloat/multilingual-e5-base) |
| Reranking | [`BAAI/bge-reranker-base`](https://huggingface.co/BAAI/bge-reranker-base), [`rozgo/bge-reranker-v2-m3`](https://huggingface.co/rozgo/bge-reranker-v2-m3), [`jinaai/jina-reranker-v1-turbo-en`](https://huggingface.co/jinaai/jina-reranker-v1-turbo-en) |
| Sparse embeddings (SPLADE) | [`prithivida/Splade_PP_en_v1`](https://huggingface.co/prithivida/Splade_PP_en_v1) |
| Late interaction (ColBERT) | [`answerdotai/answerai-colbert-small-v1`](https://huggingface.co/answerdotai/answerai-colbert-small-v1) |
| Transcription (Whisper) | [`onnx-community/whisper-tiny`](https://huggingface.co/onnx-community/whisper-tiny), [`onnx-community/whisper-base`](https://huggingface.co/onnx-community/whisper-base), [`onnx-community/whisper-small`](https://huggingface.co/onnx-community/whisper-small), [`Xenova/whisper-medium`](https://huggingface.co/Xenova/whisper-medium), [`Xenova/whisper-large-v3`](https://huggingface.co/Xenova/whisper-large-v3) |
| Tokenizers (token counting / chunk sizing) | [`Xenova/gpt-4o`](https://huggingface.co/Xenova/gpt-4o), [`thenlper/gte-small`](https://huggingface.co/thenlper/gte-small) |
| Formula recognition (LaTeX) | [`RapidAI/RapidLaTeXOCR`](https://github.com/RapidAI/RapidLaTeXOCR) (GitHub release `v0.0.0`) | Resizer + encoder + decoder ONNX and tokenizer (MIT, pix2tex family), ~180 MB, SHA256-pinned, used by the opt-in `formula-recognition` feature. |

You can point the reranker and embedding `Custom` presets at any compatible
ONNX repository on the Hub; see the [reranking](/guides/reranking/) guide.
