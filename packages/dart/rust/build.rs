use std::path::Path;

fn main() {
    // Re-run whenever any Rust source changes or FRB config changes.
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=flutter_rust_bridge.yaml");

    // Optional FRB codegen: regenerate flutter_rust_bridge artifacts when the
    // tool is on PATH. Missing tool is not fatal — committed generated sources
    // are checked in, and CI environments without FRB still build cleanly.
    match std::process::Command::new("flutter_rust_bridge_codegen")
        .args(["generate", "--config-file", "flutter_rust_bridge.yaml"])
        .status()
    {
        Ok(status) if status.success() => {
            // FRB v2.12+ emits `use` lists in an order rustfmt 2024 edition rewrites
            // (e.g. `{transform_result_dco, Lifetimeable, Lockable}` →
            // `{Lifetimeable, Lockable, transform_result_dco}`). Run rustfmt against
            // the generated file so committed output is fmt-clean and `cargo fmt --check`
            // stays green in CI.
            match std::process::Command::new("rustfmt")
                .args(["--edition", "2024", "src/frb_generated.rs"])
                .status()
            {
                Ok(s) if s.success() => {}
                Ok(s) => println!("cargo:warning=rustfmt on src/frb_generated.rs exited {s}"),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    println!(
                        "cargo:warning=rustfmt not on PATH — skipping post-FRB format. Install rustfmt via rustup to keep generated bridge sources fmt-clean."
                    );
                }
                Err(err) => println!("cargo:warning=failed to spawn rustfmt: {err}"),
            }

            // Patch the generated Dart entrypoint so the published package resolves
            // its native library from its own installed location.
            patch_published_loader();

            // Rewrite FRB-generated handler.executeSync/handler.executeNormal calls
            // into direct handler invocations. FRB 2.x emits these calls assuming
            // `handler` is a BaseHandler field, but in service-API methods `handler`
            // is a user-supplied function parameter (FutureOr<R> Function(T)) which
            // does not expose those methods, so the generated Dart fails to compile.
            // The rewrite is idempotent (marker-gated) and runs after every FRB
            // invocation — including the rebuild that fires during `dart pub get`
            // in e2e flows, which is when this otherwise reverts.
            fix_handler_executor_calls();

            // FRB is not feature-aware: it emits a wire wrapper and a dispatch arm for
            // every `pub fn` it can see, including ones behind `#[cfg(feature = ...)]`.
            // Under a reduced feature set (Android: --no-default-features --features
            // android-target) those functions are configured out and the glue fails with
            // E0425. alef injects the gates during `alef generate`, but the FRB run above
            // rewrites the file from scratch and drops them — same reversion the handler
            // rewrite above exists to survive, so this re-applies for the same reason.
            carry_frb_cfg_gates();
        }
        Ok(status) => panic!("flutter_rust_bridge_codegen generate failed (exit code: {status})"),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            println!(
                "cargo:warning=flutter_rust_bridge_codegen not on PATH — skipping codegen. Install via `cargo install flutter_rust_bridge_codegen --locked` to regenerate FRB artifacts at build time."
            );
        }
        Err(err) => panic!("failed to spawn flutter_rust_bridge_codegen: {err}"),
    }
}

const FRB_GENERATED_DART: &str = "../lib/src/xberg_bridge_generated/frb_generated.dart";
const FRB_HANDLER_EXECUTOR_MARKER: &str = "handler.executeSync(";
const LOADER_MARKER: &str = "_alefResolveExternalLibrary";
const FRB_INIT_PROLOGUE: &str = "  /// Initialize flutter_rust_bridge\n  static Future<void> init({\n    RustLibApi? api,\n    BaseHandler? handler,\n    ExternalLibrary? externalLibrary,\n    bool forceSameCodegenVersion = true,\n  }) async {\n";
const FRB_INIT_REPLACEMENT: &str = r#"  /// Resolve the prebuilt native library from the environment, the package's bundled
  /// natives, or the versioned user cache — downloading it if the cache is cold.
  ///
  /// Never returns `null`. The return type stays nullable to match flutter_rust_bridge's
  /// `init` signature, but an unresolvable native throws a descriptive `StateError`
  /// instead of deferring to FRB's default loader, whose relative-path dlopen would fail
  /// anyway under a hardened runtime.
  ///
  /// Checks in order:
  /// 1. The `nativeLibDirEnv` environment variable
  ///    (allows test harnesses to point to development build paths)
  /// 2. Package-installed location with RID subdirectory (lib/src/native/<rid>/)
  ///    (for published pub.dev packages with platform-specific bundled native libraries)
  /// 3. Package-installed location (lib/src/xberg_bridge_generated/)
  ///    (legacy fallback for development or packages without per-platform binaries)
  /// 4. Versioned user cache populated by `dart run xberg:download_libs`
  ///    (`<cache>/xberg/<version>/<rid>/`), shared with the download script
  ///    via `nativeCachedLibPath()` in `native_loader.dart`.
  /// 5. On a cache miss, downloads and caches the release asset via
  ///    `nativeDownloadAndCacheLibrary()` and opens the result.
  /// 6. Throws a StateError naming the expected release asset URL, the
  ///    download command, and the env-var override (never a silent null miss).
  static Future<ExternalLibrary?> _alefResolveExternalLibrary() async {
    try {
      const candidates = <String>[
        // macOS: framework bundle (preferred modern packaging)
        'xberg_dart.framework',
        // macOS: bare dylib fallback
        'libxberg_dart.dylib',
        // Linux
        'libxberg_dart.so',
        // Windows
        'xberg_dart.dll',
      ];

      // Helper to open a native library by absolute path.
      // Normalizes path to absolute to avoid hardened-runtime "relative path rejected" errors.
      ExternalLibrary? tryOpenAbsolute(String libPath) {
        try {
          final absPath = File(libPath).absolute.path;
          return ExternalLibrary.open(absPath);
        } catch (_) {
          return null;
        }
      }

      bool candidateExists(String libPath) {
        return File(libPath).existsSync() || Directory(libPath).existsSync();
      }

      // Check the env-var override first, so test harnesses can point at a development
      // build path. Read through `nativeLibDirEnv` rather than repeating the literal, so
      // this and the error message below can never name different variables.
      final envDir = Platform.environment[nativeLibDirEnv];
      if (envDir != null && envDir.isNotEmpty) {
        final absEnvDir = Directory(envDir).absolute.path;
        final libDir = Directory(absEnvDir);
        if (libDir.existsSync()) {
          for (final candidate in candidates) {
            final libPath = '$absEnvDir/$candidate';
            if (candidateExists(libPath)) {
              final result = tryOpenAbsolute(libPath);
              if (result != null) return result;
            }
          }
        }
      }

      // Compute RID (runtime identifier) from platform and architecture using Abi.current().
      // This is more reliable than parsing Platform.version.
      String? computeRid() {
        final abi = Abi.current();
        final os = Platform.operatingSystem;

        // Map from (os, Abi) to RID string.
        String? ridFromAbi() {
          if (os == 'linux') {
            if (abi == Abi.linuxX64) return 'linux-x64';
            if (abi == Abi.linuxArm64) return 'linux-arm64';
          } else if (os == 'macos') {
            if (abi == Abi.macosX64) return 'macos-x64';
            if (abi == Abi.macosArm64) return 'macos-arm64';
          } else if (os == 'windows') {
            if (abi == Abi.windowsX64) return 'windows-x64';
            if (abi == Abi.windowsArm64) return 'windows-arm64';
          }
          return null;
        }

        return ridFromAbi();
      }

      final rid = computeRid();
      if (rid != null) {
        final packageRoot =
            await Isolate.resolvePackageUri(Uri.parse('package:xberg/xberg.dart'));
        if (packageRoot != null) {
          final ridDir = packageRoot.resolve('src/native/$rid/');
          for (final candidate in candidates) {
            final libPath = ridDir.resolve(candidate).toFilePath();
            if (candidateExists(libPath)) {
              final result = tryOpenAbsolute(libPath);
              if (result != null) return result;
            }
          }
        }
      }

      // Check legacy package-installed location as fallback.
      final packageRoot =
          await Isolate.resolvePackageUri(Uri.parse('package:xberg/xberg.dart'));
      if (packageRoot != null) {
        final libDir = packageRoot.resolve('src/xberg_bridge_generated/');
        for (final candidate in candidates) {
          final libPath = libDir.resolve(candidate).toFilePath();
          if (candidateExists(libPath)) {
            final result = tryOpenAbsolute(libPath);
            if (result != null) return result;
          }
        }
      }

      // As a last resort, resolve the running test/script's package root via
      // `Platform.script` and search standard RID-relative locations there.
      // Critical on macOS: `Directory.current` under hardened-runtime `dart` is
      // the dart binary's own bin dir (relative-path dlopen rejected), whereas
      // `Platform.script` resolves to the running .dart file's absolute URI,
      // from which we can walk up to find the package root (the dir containing
      // `pubspec.yaml`) and look for the bundled native library at standard
      // paths. This handles the case where `Isolate.resolvePackageUri`
      // resolution did not yield the actual staging location (e.g., a path
      // dependency in local development, or a test_app whose host package
      // contains the native lib directly rather than via the bridged package).
      try {
        final scriptPath = Platform.script.toFilePath();
        var dir = File(scriptPath).absolute.parent;
        while (dir.parent.path != dir.path
            && !File('${dir.path}/pubspec.yaml').existsSync()) {
          dir = dir.parent;
        }
        if (File('${dir.path}/pubspec.yaml').existsSync()) {
          final rid = computeRid();
          final absRootPath = dir.absolute.path;
          final searchRoots = <String>[
            if (rid != null) '$absRootPath/lib/src/native/$rid',
            '$absRootPath/lib',
            absRootPath,
          ];
          for (final root in searchRoots) {
            final absRoot = Directory(root).absolute.path;
            for (final candidate in candidates) {
              final libPath = '$absRoot/$candidate';
              if (candidateExists(libPath)) {
                final result = tryOpenAbsolute(libPath);
                if (result != null) return result;
              }
            }
          }
        }
      } catch (_) {
        // fall through to the versioned-cache lookup below
      }

      // Versioned user cache populated by `dart run xberg:download_libs`.
      // Shares its cache-path logic with the download script via
      // `nativeCachedLibPath()` so the two can never disagree on the location.
      final cachedLibPath = nativeCachedLibPath();
      if (cachedLibPath != null && candidateExists(cachedLibPath)) {
        final result = tryOpenAbsolute(cachedLibPath);
        if (result != null) return result;
      }

      // Cache miss: download and cache the release asset, then open it. Any
      // failure here (network, checksum mismatch) falls through to the loud
      // StateError below rather than propagating a raw download exception.
      try {
        final downloadedPath = await nativeDownloadAndCacheLibrary();
        final result = tryOpenAbsolute(downloadedPath);
        if (result != null) return result;
      } catch (_) {
        // fall through to the descriptive miss below
      }
    } catch (_) {
      // Fall through to the descriptive miss below on any resolution failure.
    }

    // Nothing bundled and nothing staged in the cache: fail loudly rather than
    // let flutter_rust_bridge attempt a doomed relative-path dlopen. Name the
    // exact release asset, the fetch command, and the env-var override so the
    // consumer can recover.
    final rid = nativeComputeRid() ?? Platform.operatingSystem;
    throw StateError(
      'Native library for xberg ($rid) was not found. '
      'Expected it in the versioned cache (${nativeCacheDir() ?? '<unresolved cache dir>'}) '
      'or bundled with the package. Download it with '
      '`dart run xberg:download_libs`, which fetches '
      '${nativeAssetUrlBase()}.tar.gz and verifies its SHA-256, '
      'or point $nativeLibDirEnv at a directory containing the native library.',
    );
  }

  /// Initialize flutter_rust_bridge
  static Future<void> init({
    RustLibApi? api,
    BaseHandler? handler,
    ExternalLibrary? externalLibrary,
    bool forceSameCodegenVersion = true,
  }) async {
    externalLibrary ??= await _alefResolveExternalLibrary();
"#;

/// Inject the published-package native-library loader into `frb_generated.dart`.
/// Idempotent: a no-op when the marker is already present or the FRB entrypoint
/// signature is absent.
fn patch_published_loader() {
    let path = Path::new(FRB_GENERATED_DART);
    let Ok(source) = std::fs::read_to_string(path) else {
        println!(
            "cargo:warning=published-loader patch skipped: {} not found",
            FRB_GENERATED_DART
        );
        return;
    };
    if source.contains(LOADER_MARKER) {
        return;
    }
    if !source.contains(FRB_INIT_PROLOGUE) {
        println!("cargo:warning=published-loader patch skipped: FRB init prologue not found");
        return;
    }

    let mut patched = source.replacen(FRB_INIT_PROLOGUE, FRB_INIT_REPLACEMENT, 1);

    // Ensure the helper's `File`/`Isolate`/`Abi` dependencies, and the cache-aware
    // `native_loader.dart` helpers (`nativeCachedLibPath`, `nativeDownloadAndCacheLibrary`,
    // `nativeLibDirEnv`), are imported.
    for (probe, line) in [
        ("import 'dart:io';", "import 'dart:io';\n"),
        ("import 'dart:isolate';", "import 'dart:isolate';\n"),
        ("import 'dart:ffi';", "import 'dart:ffi';\n"),
        ("import '../native_loader.dart';", "import '../native_loader.dart';\n"),
    ] {
        if patched.contains(probe) {
            continue;
        }
        if let Some(pos) = patched.find("\nimport ") {
            patched.insert_str(pos + 1, line);
        } else {
            patched.insert_str(0, line);
        }
    }

    if patched != source {
        if let Err(err) = std::fs::write(path, &patched) {
            println!("cargo:warning=failed to write published-loader patch: {err}");
            return;
        }
        match std::process::Command::new("dart")
            .args(["format", FRB_GENERATED_DART])
            .status()
        {
            Ok(s) if s.success() => {}
            Ok(s) => println!("cargo:warning=dart format on {} exited {}", FRB_GENERATED_DART, s),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                println!(
                    "cargo:warning=dart not on PATH — skipping post-patch format. Install Dart SDK to keep generated FRB Dart sources fmt-clean."
                );
            }
            Err(err) => println!("cargo:warning=failed to spawn dart format: {err}"),
        }
    }
}

/// Rewrite FRB-emitted `handler.executeSync(SyncTask(...))` calls into
/// `SyncTask(...).executeSync()` form, but ONLY inside methods whose
/// signature contains a callback parameter literally named `handler`.
///
/// The check inspects the method/function signature (up to the opening brace)
/// for both the parameter name `handler` and a callback type `Function(`.
/// This avoids false positives: methods with other callback parameters
/// (e.g., named `cb`) still use the inherited base-class field, which is
/// properly callable as `.executeSync(task)` and must be left untouched.
///
/// Idempotent: if no `handler.executeSync(` marker is present, exits early.
#[allow(clippy::collapsible_if)]
fn fix_handler_executor_calls() {
    let path = Path::new(FRB_GENERATED_DART);
    let Ok(source) = std::fs::read_to_string(path) else {
        return;
    };
    if !source.contains(FRB_HANDLER_EXECUTOR_MARKER) {
        return;
    }

    let lines: Vec<&str> = source.lines().collect();
    let mut out = String::with_capacity(source.len());
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();
        let is_func_start = !trimmed.is_empty()
            && !trimmed.starts_with("//")
            && !trimmed.starts_with("class ")
            && !trimmed.starts_with("abstract class ")
            && !trimmed.starts_with("mixin ")
            && !trimmed.starts_with('}')
            && !trimmed.starts_with(')')
            && !trimmed.starts_with(']')
            && line.contains('(');
        if !is_func_start {
            out.push_str(line);
            out.push('\n');
            i += 1;
            continue;
        }
        let start = i;
        let mut paren: i32 = 0;
        let mut brace: i32 = 0;
        let mut body_started = false;
        while i < lines.len() {
            let l = lines[i];
            for c in l.chars() {
                match c {
                    '(' => paren += 1,
                    ')' => paren -= 1,
                    '{' => {
                        brace += 1;
                        body_started = true;
                    }
                    '}' => brace -= 1,
                    _ => {}
                }
            }
            i += 1;
            if body_started && brace <= 0 && paren <= 0 {
                break;
            }
        }
        let block_text = lines[start..i].join("\n");
        // Extract signature: text from method start up to and including the opening brace.
        let signature_part = extract_signature(&block_text);
        let rewritten = if signature_part.contains("handler") && signature_part.contains("Function(") {
            rewrite_executor_to_task(&block_text)
        } else {
            block_text
        };
        out.push_str(&rewritten);
        out.push('\n');
    }
    if !source.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    if out != source {
        if let Err(err) = std::fs::write(path, &out) {
            println!("cargo:warning=failed to write handler-executor rewrite: {err}");
        }
    }
}

/// Extract the method/function signature (text before and including the opening brace).
/// Tracks parentheses to handle multi-line signatures with complex parameter lists.
fn extract_signature(src: &str) -> String {
    let mut paren = 0;
    let mut result = String::new();
    for ch in src.chars() {
        match ch {
            '(' => {
                paren += 1;
                result.push(ch);
            }
            ')' => {
                result.push(ch);
                paren -= 1;
            }
            '{' if paren == 0 => {
                // Only treat `{` as body-start when all parens are closed.
                result.push(ch);
                break;
            }
            _ => result.push(ch),
        }
    }
    result
}

fn rewrite_executor_to_task(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut cursor = 0;
    while let Some((rel, method)) = next_executor_call(&src[cursor..]) {
        let start = cursor + rel;
        let open = start + "handler.".len() + method.len();
        let Some(close) = matching_paren(src, open) else {
            break;
        };
        out.push_str(&src[cursor..start]);
        let inner = src[open + 1..close].trim();
        let inner = inner.strip_suffix(',').map(str::trim_end).unwrap_or(inner);
        out.push_str(inner);
        out.push('.');
        out.push_str(method);
        out.push_str("()");
        cursor = close + 1;
    }
    out.push_str(&src[cursor..]);
    out
}

fn next_executor_call(src: &str) -> Option<(usize, &'static str)> {
    let s = src.find("handler.executeSync(");
    let n = src.find("handler.executeNormal(");
    match (s, n) {
        (Some(a), Some(b)) if a <= b => Some((a, "executeSync")),
        (Some(_), Some(b)) => Some((b, "executeNormal")),
        (Some(a), None) => Some((a, "executeSync")),
        (None, Some(b)) => Some((b, "executeNormal")),
        (None, None) => None,
    }
}

fn matching_paren(src: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (offset, ch) in src[open..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(open + offset);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_signature_stops_at_opening_brace() {
        let src = "void foo(int x) { return x; }";
        let sig = extract_signature(src);
        assert_eq!(sig, "void foo(int x) {");
    }

    #[test]
    fn extract_signature_with_callback_param() {
        let src = r#"int crateServiceApiAppConnect({
    required App that,
    required String path,
    required FutureOr<String> Function(String) cb,
  }) {
    return handler.executeSync(SyncTask(...));
  }"#;
        let sig = extract_signature(src);
        assert!(
            sig.contains("Function("),
            "signature should contain Function( for callback param"
        );
        assert!(sig.contains("{"), "signature should include opening brace");
    }

    #[test]
    fn extract_signature_without_callback_param() {
        let src = r#"void crateServiceApiAppConfig({
    required App that,
    required ServerConfig config,
  }) {
    return handler.executeSync(SyncTask(...));
  }"#;
        let sig = extract_signature(src);
        assert!(
            !sig.contains("Function("),
            "signature should not contain Function( when no callback"
        );
        assert!(sig.contains("{"), "signature should include opening brace");
    }

    #[test]
    fn extract_signature_multiline_complex_params() {
        let src = "void method(int a, String b, List<Map<String, int>> c) { /* body */ }";
        let sig = extract_signature(src);
        assert!(sig.ends_with("{"), "should end with opening brace");
        assert!(
            sig.contains("method("),
            "should contain method name and param list start"
        );
    }

    #[test]
    fn extract_signature_ignores_function_in_body() {
        // The pattern "Function(" in a comment or nested type in the body should not affect signature extraction.
        let src = r#"void process(String data) {
    // This comment mentions Function(String) but it's in the body
    return handleData();
  }"#;
        let sig = extract_signature(src);
        assert!(
            !sig.contains("Function("),
            "signature extraction should stop before body"
        );
        assert_eq!(sig, "void process(String data) {");
    }
}

const FRB_GENERATED_RUST: &str = "src/frb_generated.rs";

/// Collect `name -> #[cfg(...)]` for every column-0 `pub fn`/`pub async fn` in `lib.rs`
/// that sits directly under a column-0 `#[cfg(...)]` attribute.
///
/// Only top-level free functions matter here: those are the only items FRB turns into a
/// wire wrapper. A gate on an `impl` block is out of scope and currently unreachable.
fn cfg_gated_free_functions(lib_rs: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = lib_rs.lines().collect();
    let mut gated = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        if !lines[index].starts_with("#[cfg(") {
            index += 1;
            continue;
        }
        // Join a multi-line predicate (e.g. `any(\n feature = "a",\n ...)`) back together.
        let mut attribute = String::from(lines[index]);
        let mut cursor = index;
        while !attribute.trim_end().ends_with(")]") && cursor + 1 < lines.len() {
            cursor += 1;
            attribute.push_str(lines[cursor].trim());
        }
        // Skip any further attributes stacked on the same item.
        let mut item = cursor + 1;
        while item < lines.len() && lines[item].starts_with('#') {
            item += 1;
        }
        if let Some(rest) = lines
            .get(item)
            .and_then(|l| l.strip_prefix("pub async fn ").or_else(|| l.strip_prefix("pub fn ")))
            && let Some(name) = rest.split(['(', '<', ' ']).next()
            && !name.is_empty()
        {
            gated.push((name.to_string(), attribute));
        }
        index = cursor + 1;
    }
    gated
}

/// Re-apply `lib.rs`'s cfg gates to the FRB glue: the `wire__crate__<name>_impl`
/// definition and its numeric dispatch arm.
///
/// Dispatch arms are keyed by an explicit numeric literal fixed at generation time, not
/// by position, so removing one cannot renumber its siblings. Idempotent — a site that
/// already carries its gate is left alone.
fn carry_frb_cfg_gates() {
    let Ok(lib_rs) = std::fs::read_to_string("src/lib.rs") else {
        println!("cargo:warning=carry_frb_cfg_gates: src/lib.rs unreadable — skipping");
        return;
    };
    let Ok(generated) = std::fs::read_to_string(FRB_GENERATED_RUST) else {
        println!("cargo:warning=carry_frb_cfg_gates: {FRB_GENERATED_RUST} unreadable — skipping");
        return;
    };

    let gated = cfg_gated_free_functions(&lib_rs);
    if gated.is_empty() {
        return;
    }

    let mut lines: Vec<String> = generated.lines().map(str::to_string).collect();
    let mut applied = 0usize;
    for (name, attribute) in &gated {
        let definition = format!("fn wire__crate__{name}_impl(");
        let dispatch = format!("=> wire__crate__{name}_impl(");
        let mut index = 0;
        while index < lines.len() {
            let hit =
                lines[index].trim_start().starts_with(definition.as_str()) || lines[index].contains(dispatch.as_str());
            if hit && !lines[index.saturating_sub(1)].trim_start().starts_with("#[cfg(") {
                let indent = " ".repeat(lines[index].len() - lines[index].trim_start().len());
                lines.insert(index, format!("{indent}{attribute}"));
                applied += 1;
                index += 1;
            }
            index += 1;
        }
    }

    if applied == 0 {
        return;
    }
    let mut out = lines.join("\n");
    out.push('\n');
    if let Err(err) = std::fs::write(FRB_GENERATED_RUST, out) {
        println!("cargo:warning=carry_frb_cfg_gates: failed to write {FRB_GENERATED_RUST}: {err}");
    }
}
