# Security Policy

## Supported Versions

Security fixes are applied to the latest release on the `main` branch.
Patch releases are back-ported to the current minor series when the
vulnerability is rated High or Critical. Older minor series receive no
security back-ports.

| Version | Supported |
|---------|-----------|
| 5.x     | Yes       |
| < 5.0   | No        |

## Threat Model

Xberg is a document-extraction library. Its principal threat is
**hostile input documents** — files crafted to exhaust memory, CPU, or
disk, or to exfiltrate data from the calling process.

### Protected attack surfaces

| Threat | Mitigation |
|--------|------------|
| Decompression bombs (ZIP/OOXML/PDF) | `ZipBombValidator` enforces `SecurityLimits.max_compression_ratio` (default 100×) and `max_archive_size` (default 500 MiB) across all archive and OOXML paths. PDF embedded-file streams are checked for ratio and absolute size before recursive processing. |
| Oversized embedded files | `ExtractionConfig.max_embedded_file_bytes` (default 50 MiB) caps any single embedded attachment before recursive extraction is attempted. Applies to OOXML (DOCX/PPTX), email attachments, and PDF embedded files. |
| Runaway recursive extraction | `ExtractionConfig.max_archive_depth` (default 3) limits archive nesting depth to prevent infinite recursion on mutually-embedded documents. |
| Extraction timeout | `ExtractionConfig.extraction_timeout_secs` (default 60 s) wraps the entire extraction future in `tokio::time::timeout`. Pathological documents that take longer are cancelled with `XbergError::Timeout`. |
| Content-size bombs (repeated paragraphs) | `SecurityBudget` (`StringGrowthValidator`) enforces `SecurityLimits.max_content_size` (default 100 MiB) on accumulated element text for XML-class formats, email, and PDF. |
| XML / HTML entity expansion (billion laughs) | `EntityValidator` (per-token) and `StringGrowthValidator` (cumulative) are wired into every XML/HTML parser path. |
| Deeply nested XML / DOM depth bombs | `DepthValidator` enforces `SecurityLimits.max_xml_depth` and `max_nesting_depth` (both default 1024). |
| Table cell bombs (CSV / XLSX / HTML tables) | `TableValidator` enforces `SecurityLimits.max_table_cells` (default 100 000). |
| Path traversal in ZIP archives | `has_path_traversal()` in `extractors::security` uses `std::path::Component::ParentDir` rather than a string search, catching normalised traversal patterns. |
| DDE / external-call formula injection (Excel) | The Excel extractor scans all string cells against a regex matching `=DDE(`, `=WEBSERVICE(`, `=HYPERLINK(`, and `=cmd|`, emitting `ProcessingWarning` per match (capped at 100 per document). This is a **warning only** — it does not prevent extraction, but gives callers the information needed to reject or quarantine the file. |
| OLE compound file execution | OLE binary streams inside OOXML archives (recognised by the `D0 CF 11 E0` magic) are skipped with a `ProcessingWarning` because xberg has no safe OLE execution path. |

### Out of scope

- **Network requests**: xberg never makes outbound network requests.
  `=WEBSERVICE(...)` formulas and `=HYPERLINK(...)` cells generate
  warnings but the URLs are never resolved.
- **Macro execution**: no VBA, JavaScript, or other macro runtime exists
  inside xberg. Formula strings are read as data, not evaluated.
- **Password-protected documents**: encryption is not stripped; protected
  files are returned with an extraction error.
- **Supply-chain / dependency vulnerabilities**: report these directly to
  the dependency maintainer and open a GitHub advisory in this repo so
  we can update the pinned version.

### Configuring limits

All limits are on `ExtractionConfig.security_limits` (`SecurityLimits`
struct) and `ExtractionConfig.max_embedded_file_bytes`. The defaults are
chosen to be permissive enough for legitimate real-world documents while
blocking the most common DoS payloads. Set limits to `None` or very large
values only for input you fully trust.

```rust
use xberg::{ExtractionConfig, extractors::security::SecurityLimits};

let config = ExtractionConfig {
    // Tighten limits for untrusted input from an upload endpoint.
    security_limits: Some(SecurityLimits {
        max_content_size: 10 * 1024 * 1024,   // 10 MiB output cap
        max_compression_ratio: 50,             // 50× ratio cap
        max_table_cells: 10_000,
        ..SecurityLimits::default()
    }),
    max_embedded_file_bytes: Some(5 * 1024 * 1024), // 5 MiB per embedded file
    extraction_timeout_secs: Some(10),              // 10 s timeout
    ..ExtractionConfig::default()
};
```

## Reporting a Vulnerability

**Do not open a public GitHub issue for security vulnerabilities.**

Email both maintainers directly:

- **Na'aman Hirschfeld** — <naaman@xberg.io>
- **Tobias Silva** — <tobias@xberg.io>

You may also use GitHub's [private vulnerability
reporting](https://docs.github.com/en/code-security/security-advisories/guidance-on-reporting-and-writing-information-about-vulnerabilities/privately-reporting-a-security-vulnerability)
on this repository, which opens a private channel with the maintainers.

Include:

1. A description of the vulnerability and affected versions.
2. A minimal reproducer (if possible, a file that triggers the issue).
3. Your assessment of severity (CVSS score or plain description).
4. Whether you want public credit when the advisory is published.

We aim to acknowledge reports within **2 business days** and to publish a
fix within **14 calendar days** for Critical/High issues and **30 days**
for Medium/Low. We will coordinate disclosure timing with you.

Researchers who follow responsible disclosure will be credited in the
GitHub advisory unless they prefer to remain anonymous.

## Safe Harbor

We will not pursue legal action against researchers who:

- Make a good-faith effort to comply with this policy.
- Do not exfiltrate, modify, or destroy data beyond what is necessary to
  demonstrate the issue.
- Give us a reasonable window to remediate before public disclosure.
