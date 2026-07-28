//! Decompresses the vendored libwpd + librevenge + boost archives and
//! compiles the sources plus the C++ shim into a single static library.
//!
//! WordPerfect support targets Linux, macOS and Windows. On any other target
//! this build script is a no-op and the crate exposes stub functions (see
//! `src/lib.rs`), so wasm/android builds never pull in a C++ toolchain. That
//! decision reads `CARGO_CFG_TARGET_OS` rather than `cfg!(target_os)`, because a
//! build script is compiled for the *host*: under `--target wasm32-...` from a
//! desktop host, `cfg!` would still say "linux"/"macos" and we would try to
//! compile libwpd for wasm.
//!
//! Both libraries are built against their MPL-2.0 arm, from `.tar.gz` archives
//! committed under `vendor/` (see `vendor/PROVENANCE.md` for exact upstream
//! URLs, checksums, and how the vendored boost header subset was produced).
//! Nothing here downloads from the network or probes the system for boost:
//! librevenge and libwpd both need header-only `boost::spirit` (parsing) and
//! `boost::archive`/`boost::serialization` (the `base64_from_binary` iterator
//! librevenge uses), and `vendor/boost-subset.tar.gz` supplies exactly that
//! subset. All three archives are decompressed into `OUT_DIR` on every build.

// Host-side gate: this `cfg` mirrors the `[target.'cfg(...)'.build-dependencies]`
// block in Cargo.toml, which Cargo resolves against the host, so the module only
// exists where `cc`/`flate2`/`tar`/`sha2` are available. Whether we *do*
// anything is a separate, target-driven decision in `main`. ~keep
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
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
    // Our own `bcp` output, not a third-party download, but pinned anyway so an
    // accidental corruption of the committed archive fails the build loudly
    // (see vendor/PROVENANCE.md). ~keep
    const BOOST_SUBSET_SHA256: &str = "802ee17c5e380efbcbb696468ee3c7090aa409db89c2063b4c9b8d3e3aff1e08";

    /// vcpkg triplet used for Windows native deps across this workspace's CI
    /// (see `scripts/ci/install-system-deps/install-windows.ps1`, which
    /// installs `libheif` the same way). Only used for zlib on Windows now
    /// that boost is vendored.
    const VCPKG_TRIPLET: &str = "x64-windows-static-md";

    /// The OS we are building *for*, per Cargo. See the module docs for why this
    /// is not `cfg!(target_os)`.
    pub fn target_os() -> String {
        env::var("CARGO_CFG_TARGET_OS").unwrap_or_default()
    }

    fn targeting_windows() -> bool {
        target_os() == "windows"
    }

    /// Root of a vcpkg installation, honoring `VCPKG_ROOT`/
    /// `VCPKG_INSTALLATION_ROOT` (both set by CI), falling back to the
    /// default `C:\vcpkg` install location.
    fn vcpkg_root() -> PathBuf {
        env::var("VCPKG_ROOT")
            .or_else(|_| env::var("VCPKG_INSTALLATION_ROOT"))
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(r"C:\vcpkg"))
    }

    fn vcpkg_triplet_dir() -> PathBuf {
        vcpkg_root().join("installed").join(VCPKG_TRIPLET)
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

    /// Root of the vendored `.tar.gz` archives, relative to the crate
    /// manifest dir (see `vendor/PROVENANCE.md` for what's in each one and
    /// where it came from).
    fn vendor_dir() -> PathBuf {
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo")).join("vendor")
    }

    /// Extract `<vendor>/<archive_name>` into `out_dir`, optionally
    /// sha256-verifying the archive bytes first. Re-extracted on every build:
    /// `OUT_DIR` is not stable across builds the way a persistent cache would
    /// be, so there is nothing to skip. Returns the extracted root, i.e.
    /// `out_dir.join(expected_root)`.
    fn extract(out_dir: &Path, archive_name: &str, expected_sha256: Option<&str>, expected_root: &str) -> PathBuf {
        let archive_path = vendor_dir().join(archive_name);
        let bytes = fs::read(&archive_path).unwrap_or_else(|e| panic!("reading {archive_path:?}: {e}"));
        if let Some(sha256) = expected_sha256 {
            verify_sha256(&bytes, sha256);
        }

        let root = out_dir.join(expected_root);
        if root.exists() {
            fs::remove_dir_all(&root).ok();
        }
        let mut archive = tar::Archive::new(GzDecoder::new(&bytes[..]));
        archive
            .unpack(out_dir)
            .unwrap_or_else(|e| panic!("failed to extract {archive_name}: {e}"));
        assert!(root.is_dir(), "expected {root:?} after extracting {archive_name}");
        root
    }

    /// zlib on the `x64-windows-static-md` triplet is a static library, but its
    /// file name is not stable across vcpkg/zlib versions: classic zlib CMake
    /// emits `zlibstatic.lib` (debug `zlibstaticd.lib`), newer ports emit
    /// `zs.lib`/`zsd.lib` (see the `-lzs`/`-lzsd`/`-lzd` pkgconfig rewrites in
    /// vcpkg's zlib portfile), and some renamed variants ship
    /// `zlib.lib`/`zlibd.lib`. We probe the disk for whichever the installed
    /// cache actually produced instead of hard-coding one name. Release libs
    /// live in `<triplet>/lib`; debug libs (trailing `d`) in
    /// `<triplet>/debug/lib`.
    const ZLIB_RELEASE_STEMS: &[&str] = &["zlibstatic", "zlib", "zs", "z"];
    const ZLIB_DEBUG_STEMS: &[&str] = &["zlibstaticd", "zlibd", "zsd", "zd"];

    /// First `<stem>.lib` present in `dir`, returned as a `rustc-link-lib` stem
    /// (no `.lib` extension), trying `stems` in order.
    fn find_zlib_lib(dir: &Path, stems: &[&str]) -> Option<String> {
        stems
            .iter()
            .find(|stem| dir.join(format!("{stem}.lib")).is_file())
            .map(|stem| (*stem).to_string())
    }

    fn link_zlib() {
        if !targeting_windows() {
            // On unix, `libz-sys` (static feature) builds a `libz.a` for the
            // target and puts it on the link search path. Re-emit the link from
            // this crate — whose objects (librevenge's RVNGZipStream.o) call
            // `inflate*` — so the archive is ordered after those objects on the
            // GNU-ld command line. Relying on libz-sys's own directive alone
            // ordered the archive before the references, so ld discarded it and
            // the final binary failed with `undefined reference to inflateInit2_`.
            // Headers come from `DEP_Z_INCLUDE` (consumed in `build`).
            println!("cargo:rustc-link-lib=static=z");
            return;
        }

        // The librevenge/libwpd C++ sources are compiled by `cc` with `-MD`
        // (the release dynamic CRT — `cc` uses `-MD` regardless of the Cargo
        // profile unless `crt-static` is set), and rustc's MSVC target links
        // the release CRT even for dev builds. So we link the RELEASE vcpkg
        // zlib (`<triplet>/lib`) in *both* profiles; linking the debug zlib
        // (`<triplet>/debug/lib`, built with `-MDd`) would trip LNK2038
        // RuntimeLibrary mismatches. The debug dir is only a last-resort
        // fallback for a release-stripped cache.
        let triplet = vcpkg_triplet_dir();
        let release_lib = triplet.join("lib");
        let debug_lib = triplet.join("debug").join("lib");

        let resolved = find_zlib_lib(&release_lib, ZLIB_RELEASE_STEMS)
            .map(|stem| (release_lib.clone(), stem))
            .or_else(|| find_zlib_lib(&debug_lib, ZLIB_DEBUG_STEMS).map(|stem| (debug_lib.clone(), stem)));

        match resolved {
            Some((dir, stem)) => {
                println!("cargo:rustc-link-search=native={}", dir.display());
                println!("cargo:rustc-link-lib={stem}");
            }
            None => {
                // Nothing on disk (missing/broken vcpkg cache). Emit the
                // most-likely release name so the linker fails loudly with a
                // clear "could not open" error against the release lib dir.
                println!("cargo:rustc-link-search=native={}", release_lib.display());
                println!("cargo:rustc-link-lib=zlibstatic");
            }
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
        let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set by cargo"));

        let rev = extract(
            &out_dir,
            "librevenge-0.0.6.tar.gz",
            Some(LIBREVENGE_SHA256),
            &format!("librevenge-{LIBREVENGE_VERSION}"),
        );
        let wpd = extract(
            &out_dir,
            "libwpd-0.10.3.tar.gz",
            Some(LIBWPD_SHA256),
            &format!("libwpd-{LIBWPD_VERSION}"),
        );
        // Extracting `boost-subset.tar.gz` reproduces the `boost/boost/...`
        // layout `bcp` produces, so the include root is the extracted `boost`
        // dir itself (headers live one level below it, at
        // `boost/boost/version.hpp`).
        let boost = extract(&out_dir, "boost-subset.tar.gz", Some(BOOST_SUBSET_SHA256), "boost");

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

        // librevenge calls POSIX S_ISREG/S_ISDIR, which MSVC does not define.
        // Force-include the shim rather than patch the upstream sources. ~keep
        if targeting_windows() {
            build.flag("/FImsvc_compat.h");
            // librevenge's RVNGZipStream.cpp does `#include <zlib.h>`. On Unix
            // zlib is a system header; on Windows it lives in the vcpkg triplet
            // include dir (installed alongside the zlib we link in link_zlib).
            build.include(vcpkg_triplet_dir().join("include"));
            // Enable C++ exceptions on MSVC. Without /EHsc, MSVC leaves
            // `_CPPUNWIND` undefined, so Boost defines `BOOST_NO_EXCEPTIONS` and
            // turns `boost::throw_exception` into an undefined external the
            // vendored sources never provide (LNK2019). GCC/clang enable
            // exceptions by default, which is why only the MSVC link failed.
            build.flag("/EHsc");
        } else {
            // On unix, `libz-sys` (a unix-only dependency) built a static zlib
            // and exported its header directory as `DEP_Z_INCLUDE`. Add it so
            // librevenge's RVNGZipStream.cpp can find `<zlib.h>` even under zig
            // cross-compilation, where the host `/usr/include` is invisible to
            // the target sysroot (the failure was `zlib.h file not found`).
            if let Ok(zlib_include) = env::var("DEP_Z_INCLUDE") {
                build.include(zlib_include);
            }
        }

        for f in cpp_files(&rev.join("src/lib")) {
            build.file(f);
        }
        for f in cpp_files(&wpd.join("src/lib")) {
            // libwpd_math.cpp defines a fallback `rint` guarded to `_WIN32`, but
            // modern MSVC UCRT already provides `rint`, so compiling it triggers
            // `LNK: duplicate symbol: rint` at the final link. Skip it on Windows
            // (the UCRT symbol is used); on other targets it is an empty TU.
            if targeting_windows() && f.file_name().is_some_and(|n| n == "libwpd_math.cpp") {
                continue;
            }
            build.file(f);
        }
        build.file("src/shim.cpp");
        build.compile("xberg_libwpd");

        link_zlib();

        println!("cargo:rerun-if-changed=src/shim.cpp");
        println!("cargo:rerun-if-changed=src/msvc_compat.h");
        println!("cargo:rerun-if-changed=vendor/librevenge-0.0.6.tar.gz");
        println!("cargo:rerun-if-changed=vendor/libwpd-0.10.3.tar.gz");
        println!("cargo:rerun-if-changed=vendor/boost-subset.tar.gz");
        if targeting_windows() {
            println!("cargo:rerun-if-env-changed=VCPKG_ROOT");
            println!("cargo:rerun-if-env-changed=VCPKG_INSTALLATION_ROOT");
        }
    }
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    if matches!(build_libwpd::target_os().as_str(), "linux" | "macos" | "windows") {
        build_libwpd::build();
    }
}
