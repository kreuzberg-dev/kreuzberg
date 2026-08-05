#!/usr/bin/env bash
#   XBERG_VERSION  - Specific version to install (default: latest)

set -euo pipefail

REPO="xberg-io/xberg"
BINARY_NAME="xberg"


info() { printf '\033[1;34m%s\033[0m\n' "$*"; }
warn() { printf '\033[1;33m%s\033[0m\n' "$*" >&2; }
error() {
  printf '\033[1;31merror: %s\033[0m\n' "$*" >&2
  exit 1
}

need_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    error "need '$1' (command not found)"
  fi
}


detect_os() {
  local os
  os="$(uname -s)"
  case "$os" in
  Linux*) echo "linux" ;;
  Darwin*) echo "darwin" ;;
  *) error "unsupported OS: $os" ;;
  esac
}

detect_arch() {
  local arch
  arch="$(uname -m)"
  case "$arch" in
  x86_64 | amd64) echo "x86_64" ;;
  aarch64 | arm64) echo "aarch64" ;;
  *) error "unsupported architecture: $arch" ;;
  esac
}

detect_libc() {
  if [ -e /lib/ld-musl-x86_64.so.1 ] || [ -e /lib/ld-musl-aarch64.so.1 ]; then
    echo "musl"
  elif ldd --version 2>&1 | grep -qi musl; then
    echo "musl"
  else
    echo "gnu"
  fi
}

detect_target() {
  local os arch
  os="$(detect_os)"
  arch="$(detect_arch)"

  case "${os}-${arch}" in
  linux-x86_64) echo "x86_64-unknown-linux-$(detect_libc)" ;;
  linux-aarch64) echo "aarch64-unknown-linux-$(detect_libc)" ;;
  darwin-x86_64) echo "x86_64-apple-darwin" ;;
  darwin-aarch64) echo "aarch64-apple-darwin" ;;
  *) error "unsupported platform: ${os}-${arch}" ;;
  esac
}


get_latest_version() {
  need_cmd curl

  local url="https://api.github.com/repos/${REPO}/releases?per_page=20"
  local tag
  tag="$(curl -fsSL "$url" | grep '"tag_name"' | sed 's/.*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/' | grep '^v' | head -1 || true)"

  if [ -z "$tag" ]; then
    error "failed to fetch latest release tag from GitHub"
  fi
  echo "$tag"
}


install() {
  need_cmd curl
  need_cmd tar

  local os arch target version install_dir

  os="$(detect_os)"
  arch="$(detect_arch)"
  target="$(detect_target)"

  if [ -n "${XBERG_VERSION:-}" ]; then
    version="${XBERG_VERSION}"
    case "$version" in
    v*) ;;
    *) version="v${version}" ;;
    esac
  else
    info "Fetching latest release..."
    version="$(get_latest_version)"
  fi

  info "Installing xberg ${version} for ${target}"

  if [ -n "${XBERG_INSTALL:-}" ]; then
    install_dir="${XBERG_INSTALL}"
  elif [ "$(id -u)" -eq 0 ]; then
    install_dir="/usr/local/bin"
  else
    install_dir="${HOME}/.xberg/bin"
  fi

  mkdir -p "$install_dir"

  local artifact="xberg-cli-${target}.tar.gz"
  local url="https://github.com/${REPO}/releases/download/${version}/${artifact}"

  info "Downloading ${url}"

  tmpdir="$(mktemp -d)"
  trap 'rm -rf "$tmpdir"' EXIT

  curl -fsSL "$url" -o "${tmpdir}/${artifact}"

  tar -xzf "${tmpdir}/${artifact}" -C "$tmpdir"

  local stage_dir="${tmpdir}/xberg-cli-${target}"
  local binary_path="${stage_dir}/${BINARY_NAME}"
  if [ ! -f "$binary_path" ]; then
    error "binary not found in archive at ${binary_path}"
  fi

  cp "$binary_path" "${install_dir}/${BINARY_NAME}"
  chmod +x "${install_dir}/${BINARY_NAME}"

  if [ -f "${stage_dir}/${BINARY_NAME}.bin" ]; then
    cp "${stage_dir}/${BINARY_NAME}.bin" "${install_dir}/${BINARY_NAME}.bin"
    chmod +x "${install_dir}/${BINARY_NAME}.bin"
  fi

  if [ -d "${stage_dir}/lib" ] && [ "$(ls -A "${stage_dir}/lib" 2>/dev/null)" ]; then
    mkdir -p "${install_dir}/lib"
    # -R so nested library directories (e.g. lib/libheif) are copied, not skipped
    cp -R "${stage_dir}/lib/"* "${install_dir}/lib/"
    info "Installed runtime libraries to ${install_dir}/lib/"
  fi

  for dylib in "${stage_dir}"/*.dylib; do
    [ -f "$dylib" ] || continue
    cp "$dylib" "${install_dir}/"
    info "Installed runtime library $(basename "$dylib") to ${install_dir}/"
  done

  info "Installed ${BINARY_NAME} to ${install_dir}/${BINARY_NAME}"

  if "${install_dir}/${BINARY_NAME}" --version >/dev/null 2>&1; then
    info "Verified: $("${install_dir}/${BINARY_NAME}" --version)"
  else
    warn "Binary installed but --version check failed"
  fi

  case ":${PATH}:" in
  *":${install_dir}:"*) ;;
  *)
    warn ""
    warn "Add ${install_dir} to your PATH:"
    warn ""
    warn "  export PATH=\"${install_dir}:\$PATH\""
    warn ""
    warn "Add this to your shell profile (~/.bashrc, ~/.zshrc, etc.) to make it permanent."
    ;;
  esac
}

install
