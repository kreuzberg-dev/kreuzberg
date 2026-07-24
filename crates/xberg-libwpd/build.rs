//! Builds libwpd + librevenge from source and compiles the C++ shim into a
//! single static library.
//!
//! WordPerfect support is desktop-only. On any target other than Linux or
//! macOS this build script is a no-op and the crate exposes stub functions
//! (see `src/lib.rs`), so wasm/android/windows builds never pull in a C++
//! toolchain.
//!
//! Both libraries are built against their MPL-2.0 arm. They are downloaded from
//! their upstream release tarballs at build time (checksum-verified) and cached
//! in a workspace-relative directory (override via `XBERG_LIBWPD_CACHE_DIR`),
//! mirroring how `xberg-tesseract` provisions its native dependencies, so the
//! extraction survives `cargo clean` instead of being re-downloaded from
//! `OUT_DIR` on every build. librevenge and libwpd both require boost headers
//! at build time (header-only `boost::spirit`), which must be present on the
//! system.

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod build_libwpd {
    use flate2::read::GzDecoder;
    use sha2::{Digest, Sha256};
    use std::env;
    use std::fs;
    use std::path::{Path, PathBuf};

    const LIBREVENGE_VERSION: &str = "0.0.6";
    const LIBWPD_VERSION: &str = "0.10.3";
    const LIBREVENGE_SHA256: &str = "686cc36be3196a0a808761cfd3951a46ff809cb0e028b0902c787261a1389d0f";
    const LIBWPD_SHA256: &str = "ca3575282acff8c952c12160433ad7e73e803ff3f070b8442c7ffa1f3a19f9ae";

    fn librevenge_urls() -> Vec<String> {
        let v = LIBREVENGE_VERSION;
        vec![
            format!("https://downloads.sourceforge.net/project/libwpd/librevenge/librevenge-{v}/librevenge-{v}.tar.gz"),
            format!(
                "https://netcologne.dl.sourceforge.net/project/libwpd/librevenge/librevenge-{v}/librevenge-{v}.tar.gz"
            ),
        ]
    }

    fn libwpd_urls() -> Vec<String> {
        let v = LIBWPD_VERSION;
        vec![
            format!("https://downloads.sourceforge.net/project/libwpd/libwpd/libwpd-{v}/libwpd-{v}.tar.gz"),
            format!("https://netcologne.dl.sourceforge.net/project/libwpd/libwpd/libwpd-{v}/libwpd-{v}.tar.gz"),
        ]
    }

    /// Locate a directory containing `boost/version.hpp`. Honors
    /// `BOOST_INCLUDE_DIR`, otherwise probes the usual system locations.
    fn find_boost_include() -> PathBuf {
        if let Ok(dir) = env::var("BOOST_INCLUDE_DIR") {
            let p = PathBuf::from(dir);
            if p.join("boost/version.hpp").is_file() {
                return p;
            }
            panic!("BOOST_INCLUDE_DIR={p:?} does not contain boost/version.hpp");
        }
        let candidates = [
            "/opt/homebrew/include", // Homebrew on Apple Silicon
            "/usr/local/include",    // Homebrew on Intel / manual installs
            "/usr/include",          // Linux libboost-dev
        ];
        for c in candidates {
            if Path::new(c).join("boost/version.hpp").is_file() {
                return PathBuf::from(c);
            }
        }
        panic!(
            "boost headers not found. librevenge and libwpd need boost::spirit at \
             build time. Install boost (e.g. `brew install boost` or \
             `apt-get install libboost-dev`) or set BOOST_INCLUDE_DIR."
        );
    }

    fn download(urls: &[String]) -> Vec<u8> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .expect("failed to build HTTP client");
        let mut last_err = String::new();
        for url in urls {
            match client.get(url).send().and_then(|r| r.error_for_status()) {
                Ok(resp) => match resp.bytes() {
                    Ok(b) => return b.to_vec(),
                    Err(e) => last_err = format!("{url}: reading body: {e}"),
                },
                Err(e) => last_err = format!("{url}: {e}"),
            }
        }
        panic!("failed to download from any mirror. last error: {last_err}");
    }

    fn verify_sha256(bytes: &[u8], expected: &str) {
        let digest = Sha256::digest(bytes);
        let actual = hex(&digest);
        assert_eq!(actual, expected, "checksum mismatch: expected {expected}, got {actual}");
    }

    fn hex(bytes: &[u8]) -> String {
        let mut s = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            s.push_str(&format!("{b:02x}"));
        }
        s
    }

    /// Download, verify and extract `name-version.tar.gz` into `cache`, reusing
    /// a prior extraction when the marker file is present. Returns the extracted
    /// source root (e.g. `<cache>/libwpd-0.10.3`).
    fn provision(cache: &Path, name: &str, version: &str, urls: &[String], sha256: &str) -> PathBuf {
        let root = cache.join(format!("{name}-{version}"));
        let marker = cache.join(format!(".{name}-{version}.ok"));
        if marker.is_file() && root.is_dir() {
            return root;
        }
        let bytes = download(urls);
        verify_sha256(&bytes, sha256);
        if root.exists() {
            fs::remove_dir_all(&root).ok();
        }
        let mut archive = tar::Archive::new(GzDecoder::new(&bytes[..]));
        archive
            .unpack(cache)
            .unwrap_or_else(|e| panic!("failed to extract {name}: {e}"));
        assert!(root.is_dir(), "expected {root:?} after extracting {name}");
        fs::write(&marker, version).ok();
        root
    }

    /// Mirrors `xberg-tesseract`'s cache placement: walk up from `OUT_DIR` to
    /// the workspace's `target/` directory (4 levels: `target/<profile>/build/
    /// <crate>-<hash>/out` -> `target/`), landing the cache alongside other
    /// crates' native build artifacts instead of buried in a per-build `OUT_DIR`
    /// that gets wiped on every `cargo clean`.
    fn workspace_cache_dir_from_out_dir() -> Option<PathBuf> {
        let out_dir = env::var_os("OUT_DIR")?;
        let mut path = PathBuf::from(out_dir);
        for _ in 0..4 {
            if !path.pop() {
                return None;
            }
        }
        Some(path.join("xberg-libwpd-cache"))
    }

    /// Cache directory for the downloaded librevenge/libwpd sources: honors
    /// `XBERG_LIBWPD_CACHE_DIR`, else a workspace-relative path derived from
    /// `OUT_DIR`, else a per-OS `$HOME`-based fallback.
    fn cache_dir() -> PathBuf {
        if let Ok(custom) = env::var("XBERG_LIBWPD_CACHE_DIR") {
            return PathBuf::from(custom);
        }

        if let Some(workspace_cache) = workspace_cache_dir_from_out_dir() {
            return workspace_cache;
        }

        if cfg!(target_os = "macos") {
            let home_dir = env::var("HOME").unwrap_or_else(|_| {
                env::var("USER")
                    .map(|user| format!("/Users/{user}"))
                    .expect("neither HOME nor USER environment variable set")
            });
            PathBuf::from(home_dir)
                .join("Library")
                .join("Caches")
                .join("xberg-libwpd")
        } else {
            let home_dir = env::var("HOME").unwrap_or_else(|_| {
                env::var("USER")
                    .map(|user| format!("/home/{user}"))
                    .expect("neither HOME nor USER environment variable set")
            });
            PathBuf::from(home_dir).join(".cache").join("xberg-libwpd")
        }
    }

    fn cpp_files(dir: &Path) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("reading {dir:?}: {e}"))
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "cpp"))
            .collect();
        files.sort();
        files
    }

    pub fn build() {
        let cache = cache_dir();
        fs::create_dir_all(&cache).expect("failed to create native cache dir");

        let boost = find_boost_include();
        let rev = provision(
            &cache,
            "librevenge",
            LIBREVENGE_VERSION,
            &librevenge_urls(),
            LIBREVENGE_SHA256,
        );
        let wpd = provision(&cache, "libwpd", LIBWPD_VERSION, &libwpd_urls(), LIBWPD_SHA256);

        let mut build = cc::Build::new();
        build
            .cpp(true)
            .std("c++17")
            .warnings(false)
            .flag_if_supported("-fvisibility=hidden")
            .define("NDEBUG", None)
            .include(rev.join("inc"))
            .include(rev.join("src/lib"))
            .include(wpd.join("inc"))
            .include(wpd.join("src/lib"))
            .include(&boost)
            .include("src");

        for f in cpp_files(&rev.join("src/lib")) {
            build.file(f);
        }
        for f in cpp_files(&wpd.join("src/lib")) {
            build.file(f);
        }
        build.file("src/shim.cpp");
        build.compile("xberg_libwpd");

        // librevenge's zip stream links against system zlib.
        println!("cargo:rustc-link-lib=z");

        println!("cargo:rerun-if-changed=build.rs");
        println!("cargo:rerun-if-changed=src/shim.cpp");
        println!("cargo:rerun-if-env-changed=BOOST_INCLUDE_DIR");
        println!("cargo:rerun-if-env-changed=XBERG_LIBWPD_CACHE_DIR");
    }
}

fn main() {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    build_libwpd::build();
}
