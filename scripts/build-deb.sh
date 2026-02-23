#!/usr/bin/env bash
set -euo pipefail

# Build a Debian package that contains:
# - gnss2tec-logger binary
# - RTKLIB convbin binary built from source (GitHub tag)
# - RNXCMP rnx2crx binary built from source (pinned commit)
# - systemd service unit (runs as root)
# - default receiver config at /etc/gnss2tec-logger/ubx.dat
#
# The package intentionally stores runtime data under:
#   /var/lib/gnss2tec-logger/{data,archive}
# and does not delete that path on uninstall.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PACKAGE_NAME="gnss2tec-logger"
RTKLIB_VERSION="${RTKLIB_VERSION:-v2.4.3-b34}"
RNXCMP_REF="${RNXCMP_REF:-6d5aa31059197850f488d0a87757a2aaa7de676d}"
TARGET_TRIPLE="${TARGET_TRIPLE:-}"
DEB_ARCH="${DEB_ARCH:-}"
MAINTAINER="${MAINTAINER:-Jeremy McLynch <contact@jmclynch.org>}"
OUT_DIR="${OUT_DIR:-${ROOT_DIR}/dist}"
FORCE_REBUILD_CONVBIN="${FORCE_REBUILD_CONVBIN:-0}"
FORCE_REBUILD_RNX2CRX="${FORCE_REBUILD_RNX2CRX:-0}"
CONVBIN_CC="${CONVBIN_CC:-}"

usage() {
    cat <<'EOF'
Usage: scripts/build-deb.sh [options]

Options:
  --target <triple>             Rust target triple (for example aarch64-unknown-linux-gnu)
  --deb-arch <arch>             Debian architecture override (for example arm64, amd64)
  --out-dir <path>              Output directory for the .deb (default: ./dist)
  --rtklib-version <version>    RTKLIB git tag for convbin (default: v2.4.3-b34)
  --rnxcmp-ref <ref>            RNXCMP git ref/commit for rnx2crx (default: 6d5aa310...)
  --maintainer <text>           Maintainer field for DEBIAN/control
  -h, --help                    Show this help

Environment alternatives:
  TARGET_TRIPLE, DEB_ARCH, OUT_DIR, RTKLIB_VERSION, RNXCMP_REF, MAINTAINER,
  FORCE_REBUILD_CONVBIN=1, FORCE_REBUILD_RNX2CRX=1, CONVBIN_CC
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --target)
            TARGET_TRIPLE="$2"
            shift 2
            ;;
        --deb-arch)
            DEB_ARCH="$2"
            shift 2
            ;;
        --out-dir)
            OUT_DIR="$2"
            shift 2
            ;;
        --rtklib-version)
            RTKLIB_VERSION="$2"
            shift 2
            ;;
        --rnxcmp-ref)
            RNXCMP_REF="$2"
            shift 2
            ;;
        --maintainer)
            MAINTAINER="$2"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "Unknown option: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

map_deb_arch() {
    case "$1" in
        x86_64|x86_64-*) echo "amd64" ;;
        aarch64|aarch64-*) echo "arm64" ;;
        armv7l|armv7*-*) echo "armhf" ;;
        amd64|arm64|armhf) echo "$1" ;;
        *) return 1 ;;
    esac
}

append_git_config_kv() {
    local key="$1"
    local value="$2"
    local count="${GIT_CONFIG_COUNT:-0}"
    export "GIT_CONFIG_KEY_${count}=${key}"
    export "GIT_CONFIG_VALUE_${count}=${value}"
    export GIT_CONFIG_COUNT="$((count + 1))"
}

configure_cargo_git_transport() {
    # Some upstream dependencies define public submodules using git@github.com SSH URLs.
    # Force Cargo to use git CLI and rewrite those URLs to HTTPS for non-interactive builds.
    export CARGO_NET_GIT_FETCH_WITH_CLI="${CARGO_NET_GIT_FETCH_WITH_CLI:-true}"
    append_git_config_kv "url.https://github.com/.insteadof" "git@github.com:"
    append_git_config_kv "url.https://github.com/.insteadof" "ssh://git@github.com/"
}

configure_cross_toolchain() {
    case "${TARGET_TRIPLE}" in
        aarch64-unknown-linux-gnu)
            local linker="${CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER:-aarch64-linux-gnu-gcc}"
            if ! command -v "${linker}" >/dev/null 2>&1; then
                echo "Missing ARM64 cross linker: ${linker}" >&2
                echo "Install it (Ubuntu): sudo apt install gcc-aarch64-linux-gnu" >&2
                echo "Or build natively on an ARM64 host." >&2
                exit 1
            fi
            export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER="${linker}"
            export CC_aarch64_unknown_linux_gnu="${CC_aarch64_unknown_linux_gnu:-${linker}}"
            CONVBIN_CC="${CONVBIN_CC:-${linker}}"
            # Enable pkg-config in cross mode and point it at ARM64 pkgconfig metadata.
            export PKG_CONFIG_ALLOW_CROSS="${PKG_CONFIG_ALLOW_CROSS:-1}"
            export PKG_CONFIG_LIBDIR="${PKG_CONFIG_LIBDIR:-/usr/lib/aarch64-linux-gnu/pkgconfig:/usr/share/pkgconfig}"
            export PKG_CONFIG_PATH="${PKG_CONFIG_PATH:-/usr/lib/aarch64-linux-gnu/pkgconfig:/usr/share/pkgconfig}"
            ;;
        "")
            ;;
        *)
            ;;
    esac
}

if [[ -z "${DEB_ARCH}" ]]; then
    if [[ -n "${TARGET_TRIPLE}" ]]; then
        DEB_ARCH="$(map_deb_arch "${TARGET_TRIPLE}")" || {
            echo "Could not infer Debian architecture from target: ${TARGET_TRIPLE}" >&2
            exit 1
        }
    else
        HOST_ARCH="$(uname -m)"
        DEB_ARCH="$(map_deb_arch "${HOST_ARCH}")" || {
            echo "Could not infer Debian architecture from host: ${HOST_ARCH}" >&2
            exit 1
        }
    fi
fi

if ! command -v cargo >/dev/null 2>&1; then
    echo "cargo is required but not found in PATH" >&2
    exit 1
fi
if ! command -v dpkg-deb >/dev/null 2>&1; then
    echo "dpkg-deb is required but not found in PATH" >&2
    exit 1
fi
if ! command -v curl >/dev/null 2>&1; then
    echo "curl is required but not found in PATH" >&2
    exit 1
fi
if ! command -v make >/dev/null 2>&1; then
    echo "make is required but not found in PATH" >&2
    exit 1
fi

if [[ ! -f "${ROOT_DIR}/Cargo.lock" ]]; then
    echo "Cargo.lock is missing. Commit Cargo.lock so --locked builds can run in CI." >&2
    exit 1
fi

if [[ -n "${TARGET_TRIPLE}" ]]; then
    configure_cross_toolchain
fi
configure_cargo_git_transport

APP_VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "${ROOT_DIR}/Cargo.toml" | head -n 1)"
if [[ -z "${APP_VERSION}" ]]; then
    echo "Could not read package version from Cargo.toml" >&2
    exit 1
fi

pushd "${ROOT_DIR}" >/dev/null

# 1) Build the main logger binary.
BUILD_ARGS=(build --release --locked --bin "${PACKAGE_NAME}")
if [[ -n "${TARGET_TRIPLE}" ]]; then
    BUILD_ARGS+=(--target "${TARGET_TRIPLE}")
fi
cargo "${BUILD_ARGS[@]}"

if [[ -n "${TARGET_TRIPLE}" ]]; then
    LOGGER_BIN="${ROOT_DIR}/target/${TARGET_TRIPLE}/release/${PACKAGE_NAME}"
else
    LOGGER_BIN="${ROOT_DIR}/target/release/${PACKAGE_NAME}"
fi
if [[ ! -x "${LOGGER_BIN}" ]]; then
    echo "Logger binary not found after build: ${LOGGER_BIN}" >&2
    exit 1
fi

# 2) Build and install convbin from RTKLIB sources into local tool root.
TOOLS_ROOT="${ROOT_DIR}/target/package-tools/${TARGET_TRIPLE:-host}"
if [[ -z "${CONVBIN_CC}" ]]; then
    CONVBIN_CC="${CC:-gcc}"
fi
if ! command -v "${CONVBIN_CC}" >/dev/null 2>&1; then
    echo "convbin compiler not found: ${CONVBIN_CC}" >&2
    exit 1
fi

RTKLIB_SRC_ROOT="${ROOT_DIR}/target/package-tools/rtklib-src/${RTKLIB_VERSION}"
CONVBIN_BIN="${TOOLS_ROOT}/bin/convbin"

if [[ ! -x "${CONVBIN_BIN}" || "${FORCE_REBUILD_CONVBIN}" = "1" ]]; then
    if [[ ! -d "${RTKLIB_SRC_ROOT}/.git" || "${FORCE_REBUILD_CONVBIN}" = "1" ]]; then
        rm -rf "${RTKLIB_SRC_ROOT}"
        git clone --depth 1 --branch "${RTKLIB_VERSION}" \
            https://github.com/tomojitakasu/RTKLIB.git "${RTKLIB_SRC_ROOT}"
    fi
    if [[ -d "${RTKLIB_SRC_ROOT}/app/consapp/convbin/gcc" ]]; then
        CONVBIN_BUILD_DIR="${RTKLIB_SRC_ROOT}/app/consapp/convbin/gcc"
    elif [[ -d "${RTKLIB_SRC_ROOT}/app/convbin/gcc" ]]; then
        CONVBIN_BUILD_DIR="${RTKLIB_SRC_ROOT}/app/convbin/gcc"
    else
        echo "convbin build directory not found in RTKLIB source tree: ${RTKLIB_SRC_ROOT}" >&2
        exit 1
    fi
    make -C "${CONVBIN_BUILD_DIR}" clean
    make -C "${CONVBIN_BUILD_DIR}" CC="${CONVBIN_CC}" convbin
    install -d -m 0755 "${TOOLS_ROOT}/bin"
    install -m 0755 "${CONVBIN_BUILD_DIR}/convbin" "${CONVBIN_BIN}"
fi

if [[ ! -x "${CONVBIN_BIN}" ]]; then
    echo "convbin binary not found after build: ${CONVBIN_BIN}" >&2
    exit 1
fi

# 3) Build and install rnx2crx from RNXCMP sources into local tool root.
RNXCMP_SRC_ROOT="${ROOT_DIR}/target/package-tools/rnxcmp-src/${RNXCMP_REF}"
RNX2CRX_BIN="${TOOLS_ROOT}/bin/rnx2crx"

if [[ ! -x "${RNX2CRX_BIN}" || "${FORCE_REBUILD_RNX2CRX}" = "1" ]]; then
    rm -rf "${RNXCMP_SRC_ROOT}"
    install -d -m 0755 "${RNXCMP_SRC_ROOT}"
    install -d -m 0755 "${TOOLS_ROOT}/bin"
    RNXCMP_ARCHIVE="${ROOT_DIR}/target/package-tools/rnxcmp-${RNXCMP_REF}.tar.gz"
    curl -fsSL "https://codeload.github.com/lhuisman/rnxcmp/tar.gz/${RNXCMP_REF}" \
        -o "${RNXCMP_ARCHIVE}"
    tar -xzf "${RNXCMP_ARCHIVE}" --strip-components=1 -C "${RNXCMP_SRC_ROOT}"
    "${CONVBIN_CC}" -O2 -o "${RNX2CRX_BIN}" "${RNXCMP_SRC_ROOT}/source/rnx2crx.c"
fi

if [[ ! -x "${RNX2CRX_BIN}" ]]; then
    echo "rnx2crx binary not found after build: ${RNX2CRX_BIN}" >&2
    exit 1
fi

# 4) Assemble Debian package root filesystem.
STAGING_ROOT="${ROOT_DIR}/target/deb-staging"
PKG_DIR="${STAGING_ROOT}/${PACKAGE_NAME}_${APP_VERSION}_${DEB_ARCH}"
rm -rf "${PKG_DIR}"

install -d -m 0755 \
    "${PKG_DIR}/DEBIAN" \
    "${PKG_DIR}/usr/bin" \
    "${PKG_DIR}/usr/lib/gnss2tec-logger/bin" \
    "${PKG_DIR}/usr/share/doc/gnss2tec-logger" \
    "${PKG_DIR}/etc/gnss2tec-logger" \
    "${PKG_DIR}/lib/systemd/system"

install -m 0755 "${LOGGER_BIN}" "${PKG_DIR}/usr/bin/${PACKAGE_NAME}"
install -m 0755 "${CONVBIN_BIN}" "${PKG_DIR}/usr/lib/gnss2tec-logger/bin/convbin"
install -m 0755 "${RNX2CRX_BIN}" "${PKG_DIR}/usr/lib/gnss2tec-logger/bin/rnx2crx"
install -m 0644 "${RTKLIB_SRC_ROOT}/readme.txt" \
    "${PKG_DIR}/usr/share/doc/gnss2tec-logger/RTKLIB_README.txt"
install -m 0644 "${RNXCMP_SRC_ROOT}/docs/README.txt" \
    "${PKG_DIR}/usr/share/doc/gnss2tec-logger/RNXCMP_README.txt"
install -m 0644 "${ROOT_DIR}/packaging/config/ubx.dat" "${PKG_DIR}/etc/gnss2tec-logger/ubx.dat"
install -m 0644 "${ROOT_DIR}/packaging/config/runtime.env" "${PKG_DIR}/etc/gnss2tec-logger/runtime.env"
install -m 0644 "${ROOT_DIR}/packaging/systemd/gnss2tec-logger.service" \
    "${PKG_DIR}/lib/systemd/system/gnss2tec-logger.service"

install -m 0755 "${ROOT_DIR}/packaging/debian/postinst" "${PKG_DIR}/DEBIAN/postinst"
install -m 0755 "${ROOT_DIR}/packaging/debian/prerm" "${PKG_DIR}/DEBIAN/prerm"
install -m 0755 "${ROOT_DIR}/packaging/debian/postrm" "${PKG_DIR}/DEBIAN/postrm"

INSTALLED_SIZE="$(du -sk "${PKG_DIR}" | cut -f 1)"
cat > "${PKG_DIR}/DEBIAN/control" <<EOF
Package: ${PACKAGE_NAME}
Version: ${APP_VERSION}
Section: science
Priority: optional
Architecture: ${DEB_ARCH}
Maintainer: ${MAINTAINER}
Depends: systemd
Installed-Size: ${INSTALLED_SIZE}
Description: GNSS UBX logger with hourly RINEX conversion
 Logs UBX data from a GNSS receiver and performs hourly conversion into
 compressed RINEX products using bundled RTKLIB convbin and RNXCMP rnx2crx.
EOF

# Keep local receiver config changes across package upgrades/removal.
cat > "${PKG_DIR}/DEBIAN/conffiles" <<'EOF'
/etc/gnss2tec-logger/ubx.dat
/etc/gnss2tec-logger/runtime.env
EOF

# 5) Build the final .deb artifact.
mkdir -p "${OUT_DIR}"
DEB_PATH="${OUT_DIR}/${PACKAGE_NAME}_${APP_VERSION}_${DEB_ARCH}.deb"
dpkg-deb --root-owner-group --build "${PKG_DIR}" "${DEB_PATH}"

popd >/dev/null
echo "Built package: ${DEB_PATH}"
