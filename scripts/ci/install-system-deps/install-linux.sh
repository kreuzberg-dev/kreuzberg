#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${REPO_ROOT:-$(cd "$SCRIPT_DIR/../../.." && pwd)}"

source "$REPO_ROOT/scripts/lib/retry.sh"

echo "::group::Installing Linux dependencies"

echo "Updating package index..."
if ! retry_with_backoff sudo apt-get update; then
  echo "::warning::apt-get update failed after retries, continuing anyway..."
fi

packages=(
  tesseract-ocr
  tesseract-ocr-eng
  tesseract-ocr-tur
  tesseract-ocr-deu
  # Korean and vertical-Japanese packs back the multilingual PNG (eng+kor) and
  # vertical JPEG (jpn_vert) OCR benchmark fixtures. The harness OCR preflight
  # fails fast unless these resolve on disk, so they must be installed here
  # rather than downloaded inside the timed extraction. ~keep
  tesseract-ocr-kor
  tesseract-ocr-jpn-vert
  fonts-liberation
  fonts-dejavu-core
  fonts-noto-core
  libssl-dev
  pkg-config
  build-essential
  patchelf
  cmake
  libmagic-dev
  libuv1-dev
  libde265-dev
  libaom-dev
  libx265-dev
  libdav1d-dev
  libnuma-dev
  # boost (header-only spirit) and zlib headers are build-time deps of
  # librevenge + libwpd, compiled from source by the xberg-libwpd crate. ~keep
  libboost-dev
  zlib1g-dev
  # liblzma-dev provides the liblzma.so linker symlink. The swift package
  # statically links libxberg_ffi.a, whose lzma-sys transitive dep surfaces
  # `-llzma` at the swift link step; the runner ships liblzma5 (runtime) but
  # not the dev symlink, so ld.gold fails with "cannot find -llzma". ~keep
  liblzma-dev
  # libbz2-dev provides the libbz2.so linker symlink. Same reasoning as
  # liblzma-dev: the swift package's bzip2 crates (archive/zip/unhwp paths)
  # emit `-lbz2`; the runner ships libbz2 runtime but not the dev symlink, so
  # ld.gold fails with "cannot find -lbz2". ~keep
  libbz2-dev
)

echo "Installing dependencies..."
if retry_with_backoff_timeout 900 sudo apt-get install -y "${packages[@]}"; then
  echo "✓ All packages installed successfully"
else
  exit_code=$?
  if [ $exit_code -eq 124 ]; then
    echo "::error::Package installation timed out after 15 minutes"
  else
    echo "::warning::Some packages failed to install, attempting individual installs..."
    for pkg in tesseract-ocr libssl-dev pkg-config cmake; do
      echo "Installing $pkg..."
      if retry_with_backoff_timeout 300 sudo apt-get install -y "$pkg" 2>&1; then
        echo "  ✓ $pkg installed"
      else
        echo "  ⚠ Failed to install $pkg"
      fi
    done
  fi
fi

# Install PHP dev headers only when no PHP is already active. The php-extension
# build matrix runs shivammathur/setup-php first (matrix.php), putting phpX.Y and
# its -dev headers on PATH; apt's unversioned php-cli/php-dev pull Ubuntu Noble's
# default (8.3) and reset php-config, so ext-php-rs builds against the wrong PHP
# version. Guard on `command -v php`, mirroring the Windows/macOS scripts. ~keep
if command -v php >/dev/null 2>&1; then
  echo "✓ PHP already active: $(php --version | head -1)"
else
  echo "Installing PHP (php-cli, php-dev)..."
  retry_with_backoff_timeout 300 sudo apt-get install -y php-cli php-dev ||
    echo "::warning::Failed to install php-cli/php-dev"
fi

echo "::endgroup::"

echo "::group::Building libheif from source (Noble ships 1.17.6, libheif-sys needs >=1.21)"

LIBHEIF_VERSION="${LIBHEIF_VERSION:-1.23.0}"
LIBHEIF_PREFIX="${LIBHEIF_PREFIX:-/usr/local}"

echo "Removing apt's libheif to prevent shadowing..."
if dpkg -l | grep -q "^ii.*libheif"; then
  sudo apt-get remove -y libheif* || echo "::warning::Failed to remove apt libheif, continuing..."
else
  echo "✓ apt libheif not installed"
fi

LIBHEIF_MARKER="$LIBHEIF_PREFIX/lib/pkgconfig/libheif.pc"

if [ -f "$LIBHEIF_MARKER" ] && pkg-config --modversion libheif 2>/dev/null | grep -q "^${LIBHEIF_VERSION}$"; then
  echo "✓ libheif ${LIBHEIF_VERSION} already installed (cached)"
else
  echo "Building libheif ${LIBHEIF_VERSION} from source..."
  build_dir="$(mktemp -d)"
  pushd "$build_dir" >/dev/null

  if retry_with_backoff_timeout 300 curl -fsSL -o libheif.tar.gz \
    "https://github.com/strukturag/libheif/releases/download/v${LIBHEIF_VERSION}/libheif-${LIBHEIF_VERSION}.tar.gz"; then
    tar xzf libheif.tar.gz
    cd "libheif-${LIBHEIF_VERSION}"
    mkdir build
    cd build
    cmake .. \
      -DCMAKE_BUILD_TYPE=Release \
      -DCMAKE_INSTALL_PREFIX="$LIBHEIF_PREFIX" \
      -DCMAKE_INSTALL_LIBDIR=lib \
      -DWITH_EXAMPLES=OFF \
      -DWITH_GDK_PIXBUF=OFF \
      -DBUILD_TESTING=OFF
    make -j"$(nproc)"
    sudo make install
    echo "✓ libheif ${LIBHEIF_VERSION} installed to $LIBHEIF_PREFIX"
  else
    echo "::error::Failed to download libheif source"
    exit 1
  fi

  popd >/dev/null
  rm -rf "$build_dir"
fi

sudo ldconfig

if [ -n "${GITHUB_ACTION:-}" ]; then
  mkdir -p /tmp/libheif-cache/usr/local/lib/pkgconfig
  mkdir -p /tmp/libheif-cache/usr/local/include
  mkdir -p /tmp/libheif-cache/usr/local/share
  cp -a /usr/local/lib/libheif* /tmp/libheif-cache/usr/local/lib/ 2>/dev/null || true
  cp -a /usr/local/lib/pkgconfig/libheif.pc /tmp/libheif-cache/usr/local/lib/pkgconfig/ 2>/dev/null || true
  [ -d /usr/local/include/libheif ] && cp -a /usr/local/include/libheif /tmp/libheif-cache/usr/local/include/
  [ -d /usr/local/share/libheif ] && cp -a /usr/local/share/libheif /tmp/libheif-cache/usr/local/share/
fi

echo ""
echo "libheif symbol verification:"
if [ -f "$LIBHEIF_PREFIX/lib/libheif.so.1" ]; then
  if nm -D "$LIBHEIF_PREFIX/lib/libheif.so.1" 2>/dev/null | grep -q "heif_image_get_plane_readonly2"; then
    echo "✓ $LIBHEIF_PREFIX/lib/libheif.so.1 has heif_image_get_plane_readonly2"
  else
    echo "::warning::$LIBHEIF_PREFIX/lib/libheif.so.1 missing heif_image_get_plane_readonly2"
  fi
else
  echo "::warning::$LIBHEIF_PREFIX/lib/libheif.so.1 not found"
fi

echo "ldconfig cache contains:"
ldconfig -p | grep libheif || echo "(no libheif in ldconfig cache)"

if [[ -n "${GITHUB_ENV:-}" ]]; then
  echo "PKG_CONFIG_PATH=$LIBHEIF_PREFIX/lib/pkgconfig:${PKG_CONFIG_PATH:-}" >>"$GITHUB_ENV"
  echo "LD_LIBRARY_PATH=$LIBHEIF_PREFIX/lib:${LD_LIBRARY_PATH:-}" >>"$GITHUB_ENV"
fi

echo "::endgroup::"

echo "::group::Verifying Linux installations"

echo "CMake:"
if command -v cmake >/dev/null 2>&1; then
  cmake --version | head -1
  echo "✓ CMake available"
  CMAKE_FULL_PATH="$(command -v cmake)"
  if [[ -n "$GITHUB_ENV" ]]; then
    echo "CMAKE=$CMAKE_FULL_PATH" >>"$GITHUB_ENV"
    echo "✓ Set CMAKE=$CMAKE_FULL_PATH in GITHUB_ENV"
  fi
  CMAKE_BIN="$(dirname "$CMAKE_FULL_PATH")"
  if [[ -n "$GITHUB_PATH" && -d "$CMAKE_BIN" ]]; then
    echo "$CMAKE_BIN" >>"$GITHUB_PATH"
    echo "✓ Added cmake directory to GITHUB_PATH: $CMAKE_BIN"
  fi
else
  echo "::error::CMake not found after installation"
  exit 1
fi

echo ""
echo "Tesseract:"
if command -v tesseract >/dev/null 2>&1; then
  # Full output, not `head -1`: `tesseract --version` reports the engine
  # version on line 1 but the *compiled* leptonica version plus the linked
  # image-codec library versions (libpng/libjpeg/zlib/...) on the following
  # lines. Truncating to line 1 was throwing away exactly the evidence needed
  # to tell "same tesseract, different leptonica" apart from "same everything".
  if tesseract --version 2>&1; then
    echo "✓ Tesseract CLI available"
  else
    echo "::warning::Tesseract CLI present but failed to run"
  fi
else
  echo "::warning::Tesseract CLI not found; continuing (OCR will rely on bundled Tesseract)"
fi

# --- diagnostics for task #494 (word_language_is_forwarded_per_ocr_element /
# paragraph-metadata: an arch-dependent Tesseract FFI declination, observed on
# x86_64-linux CI only, that production code already treats as legitimate --
# see crates/xberg/tests/issue_177_180_189_ocr_metadata.rs). The install
# script previously asserted only that the same *apt install command* ran on
# every arch, never what it actually resolved to. apt mirrors serve
# independent per-architecture package builds, so "same command" does not
# imply "same tesseract-ocr/libtesseract/liblept build or even version" --
# print the resolved package versions and binary checksums explicitly so two
# runs (ubuntu-latest vs ubuntu-24.04-arm) can be diffed directly instead of
# re-deriving this from a guess. ~keep
echo ""
echo "Runner identity (for cross-arch diffing):"
uname -a
[ -f /etc/os-release ] && cat /etc/os-release

echo ""
echo "Resolved package versions (tesseract/leptonica), this runner:"
dpkg-query -W -f='${Package}\t${Version}\t${Architecture}\n' \
  'tesseract-ocr*' 'libtesseract*' 'liblept*' 2>/dev/null ||
  echo "(dpkg-query found no matching packages)"

echo ""
echo "Available Tesseract languages (full list, not truncated):"
if command -v tesseract >/dev/null 2>&1; then
  tesseract --list-langs || true
else
  echo "(tesseract CLI not available)"
fi

echo ""
echo "PHP:"
if command -v php >/dev/null 2>&1; then
  php --version | head -1
  echo "✓ PHP available"
else
  echo "::error::PHP not found after installation"
  exit 1
fi

echo ""
echo "Checking Tesseract data path..."

tessdata_found=0
for tessdata_path in "/usr/share/tesseract-ocr/5/tessdata" "/usr/share/tesseract-ocr/tessdata"; do
  if [ -d "$tessdata_path" ]; then
    echo "Found tessdata at: $tessdata_path"

    echo "Required language files:"
    for lang in eng tur deu; do
      if [ -f "$tessdata_path/${lang}.traineddata" ]; then
        size=$(stat -c%s "$tessdata_path/${lang}.traineddata" 2>/dev/null || echo "unknown")
        # sha256, not just size: two traineddata files can happen to match in
        # byte count while differing in content. A byte-identical hash across
        # the x86_64 and aarch64 runner logs is the strongest available proof
        # that "same language pack" is actually true rather than assumed --
        # if the hashes differ, the model data itself is the divergence, not
        # a Tesseract/leptonica code-version skew.
        checksum=$(sha256sum "$tessdata_path/${lang}.traineddata" 2>/dev/null | awk '{print $1}')
        echo "  ✓ ${lang}.traineddata ($size bytes, sha256 ${checksum:-unknown})"
      else
        echo "  ⚠ ${lang}.traineddata (missing)"
      fi
    done
    tessdata_found=1
    break
  fi
done

if [ $tessdata_found -eq 0 ]; then
  echo "::error::Tessdata directory not found in standard locations"
  exit 1
fi

echo "::endgroup::"
