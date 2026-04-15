#!/bin/sh
# shellcheck disable=SC3043  # 'local' is supported by dash, ash, and busybox sh
set -eu

# --- Logging ---

info() {
    printf 'creft: %s\n' "$*" >&2
}

warn() {
    printf 'creft: warning: %s\n' "$*" >&2
}

error() {
    printf 'creft: error: %s\n' "$*" >&2
    exit 1
}

debug() {
    if [ "${CREFT_DEBUG:-}" = "1" ]; then
        printf 'creft: debug: %s\n' "$*" >&2
    fi
}

# --- Platform detection ---

get_os() {
    local os
    os="$(uname -s)"
    case "$os" in
        Darwin) printf 'darwin' ;;
        Linux)  printf 'linux' ;;
        *)      error "unsupported OS: $os" ;;
    esac
}

get_arch() {
    local arch
    arch="$(uname -m)"
    case "$arch" in
        x86_64)           printf 'x86_64' ;;
        aarch64 | arm64) printf 'aarch64' ;;
        *)                error "unsupported architecture: $arch" ;;
    esac
}

get_target() {
    local os arch
    os="$(get_os)"
    arch="$(get_arch)"
    case "${os}-${arch}" in
        darwin-aarch64) printf 'aarch64-apple-darwin' ;;
        darwin-x86_64)  printf 'x86_64-apple-darwin' ;;
        linux-x86_64)   printf 'x86_64-unknown-linux-gnu' ;;
        linux-aarch64)  printf 'aarch64-unknown-linux-gnu' ;;
        *)              error "unsupported platform: ${os}-${arch}" ;;
    esac
}

# --- Version resolution ---

get_latest_version() {
    local api_url response version
    api_url="https://api.github.com/repos/chrisfentiman/creft/releases/latest"
    debug "querying $api_url"
    response="$(download_to_stdout "$api_url")" || error "failed to determine latest version"
    version="$(printf '%s' "$response" | grep '"tag_name"' | sed 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/')"
    if [ -z "$version" ]; then
        error "failed to determine latest version"
    fi
    printf '%s' "$version"
}

# --- Download helpers ---

check_downloader() {
    if command -v curl >/dev/null 2>&1; then
        printf 'curl'
    elif command -v wget >/dev/null 2>&1; then
        printf 'wget'
    else
        error "curl or wget is required but neither is installed"
    fi
}

github_auth_header() {
    local token
    token="${GITHUB_TOKEN:-${GH_TOKEN:-}}"
    if [ -n "$token" ]; then
        printf 'Authorization: token %s' "$token"
    fi
}

download_to_stdout() {
    local url downloader auth_header
    url="$1"
    downloader="$(check_downloader)"
    auth_header="$(github_auth_header)"
    case "$downloader" in
        curl)
            if [ -n "$auth_header" ]; then
                curl --proto '=https' --tlsv1.2 -fsSL -H "$auth_header" "$url"
            else
                curl --proto '=https' --tlsv1.2 -fsSL "$url"
            fi
            ;;
        wget)
            if [ -n "$auth_header" ]; then
                wget -qO- --header "$auth_header" "$url"
            else
                wget -qO- "$url"
            fi
            ;;
    esac
}

download_to_file() {
    local url dest downloader auth_header
    url="$1"
    dest="$2"
    downloader="$(check_downloader)"
    auth_header="$(github_auth_header)"
    case "$downloader" in
        curl)
            if [ -n "$auth_header" ]; then
                curl --proto '=https' --tlsv1.2 -fSL -H "$auth_header" -o "$dest" "$url"
            else
                curl --proto '=https' --tlsv1.2 -fSL -o "$dest" "$url"
            fi
            ;;
        wget)
            if [ -n "$auth_header" ]; then
                wget -q -O "$dest" --header "$auth_header" "$url"
            else
                wget -q -O "$dest" "$url"
            fi
            ;;
    esac
}

# --- Checksum ---

shasum_cmd() {
    if command -v shasum >/dev/null 2>&1; then
        printf 'shasum -a 256'
    elif command -v sha256sum >/dev/null 2>&1; then
        printf 'sha256sum'
    else
        error "shasum or sha256sum is required but neither is installed"
    fi
}

verify_checksum() {
    local tarball checksum_url checksum_file expected actual cmd
    tarball="$1"
    checksum_url="$2"
    checksum_file="${tarball}.sha256"

    debug "downloading checksum from $checksum_url"
    download_to_file "$checksum_url" "$checksum_file" || error "failed to download checksum"

    # The .sha256 file contains "<hash>  <filename>" — extract just the hash
    expected="$(awk '{print $1}' "$checksum_file")"
    if [ -z "$expected" ]; then
        error "checksum verification failed"
    fi

    cmd="$(shasum_cmd)"
    # Compute hash of the tarball
    actual="$($cmd "$tarball" | awk '{print $1}')"

    if [ "$expected" != "$actual" ]; then
        error "checksum verification failed"
    fi
    debug "checksum verified: $actual"
}

# --- Install ---

install_creft() {
    local version tag target tarball_name tarball_url checksum_url install_dir

    # Resolve version
    if [ -n "${CREFT_VERSION:-}" ]; then
        version="$CREFT_VERSION"
        info "using pinned version $version"
    else
        info "resolving latest version..."
        version="$(get_latest_version)"
        info "latest version is $version"
    fi

    # Normalize to a release tag — accept either "0.2.8" or "creft-v0.2.8"
    case "$version" in
        creft-v*) tag="$version" ;;
        v*)       tag="creft-${version}" ;;
        *)        tag="creft-v${version}" ;;
    esac

    target="$(get_target)"
    debug "target triple: $target"

    tarball_name="creft-${target}.tar.gz"
    tarball_url="https://github.com/chrisfentiman/creft/releases/download/${tag}/${tarball_name}"
    checksum_url="${tarball_url}.sha256"

    install_dir="${CREFT_INSTALL_DIR:-$HOME/.local/bin}"

    # Set up temp dir — declared at script scope so the EXIT trap can reference it
    _creft_tmp_dir="$(mktemp -d)"
    trap 'rm -rf "${_creft_tmp_dir:-}"' EXIT

    local tarball
    tarball="${_creft_tmp_dir}/${tarball_name}"

    info "downloading creft ${version} for ${target}..."
    download_to_file "$tarball_url" "$tarball" || error "failed to download creft ${version} for ${target}"

    info "verifying checksum..."
    verify_checksum "$tarball" "$checksum_url"

    info "extracting..."
    tar -xzf "$tarball" -C "$_creft_tmp_dir" || error "failed to extract archive"

    mkdir -p "$install_dir"
    cp "${_creft_tmp_dir}/creft" "${install_dir}/creft"
    chmod +x "${install_dir}/creft"

    info "creft ${version} installed to ${install_dir}/creft"

    check_path "$install_dir"

    # Show welcome on first install — use full path since install_dir may not be on PATH yet
    "${install_dir}/creft" _creft welcome || true
}

# --- PATH check ---

check_path() {
    local install_dir
    install_dir="$1"

    # Check if install_dir appears in PATH
    case ":${PATH}:" in
        *":${install_dir}:"*) ;;
        *)
            warn "${install_dir} is not in your PATH. Add it with:"
            warn "  export PATH=\"${install_dir}:\$PATH\""
            ;;
    esac
}

# --- Entry point ---

install_creft
