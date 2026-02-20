#!/usr/bin/env bash
set -euo pipefail

# Build a Debian package that contains:
# - gnss2tec-logger binary
# - ubx2rinex binary built from source (crates.io)
# - systemd service unit (runs as root)
# - default receiver config at /etc/gnss2tec-logger/ubx.dat
#
# The package intentionally stores runtime data under:
#   /var/lib/gnss2tec-logger/{data,archive}
# and does not delete that path on uninstall.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PACKAGE_NAME="gnss2tec-logger"
UBX2RINEX_VERSION="${UBX2RINEX_VERSION:-0.3.0}"
TARGET_TRIPLE="${TARGET_TRIPLE:-}"
DEB_ARCH="${DEB_ARCH:-}"
MAINTAINER="${MAINTAINER:-GNSS2TEC Logger Maintainers <maintainers@example.com>}"
OUT_DIR="${OUT_DIR:-${ROOT_DIR}/dist}"
FORCE_REBUILD_UBX2RINEX="${FORCE_REBUILD_UBX2RINEX:-0}"

usage() {
    cat <<'EOF'
Usage: scripts/build-deb.sh [options]

Options:
  --target <triple>             Rust target triple (for example aarch64-unknown-linux-gnu)
  --deb-arch <arch>             Debian architecture override (for example arm64, amd64)
  --out-dir <path>              Output directory for the .deb (default: ./dist)
  --ubx2rinex-version <version> ubx2rinex crate version (default: 0.3.0)
  --maintainer <text>           Maintainer field for DEBIAN/control
  -h, --help                    Show this help

Environment alternatives:
  TARGET_TRIPLE, DEB_ARCH, OUT_DIR, UBX2RINEX_VERSION, MAINTAINER,
  FORCE_REBUILD_UBX2RINEX=1
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
        --ubx2rinex-version)
            UBX2RINEX_VERSION="$2"
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

if [[ ! -f "${ROOT_DIR}/Cargo.lock" ]]; then
    echo "Cargo.lock is missing. Commit Cargo.lock so --locked builds can run in CI." >&2
    exit 1
fi

if [[ -n "${TARGET_TRIPLE}" ]]; then
    configure_cross_toolchain
fi

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

# 2) Build and install ubx2rinex from source into a local tool root.
TOOLS_ROOT="${ROOT_DIR}/target/package-tools/${TARGET_TRIPLE:-host}"
UBX2RINEX_BIN="${TOOLS_ROOT}/bin/ubx2rinex"
if [[ ! -x "${UBX2RINEX_BIN}" || "${FORCE_REBUILD_UBX2RINEX}" = "1" ]]; then
    INSTALL_ARGS=(install --locked --force --root "${TOOLS_ROOT}" --version "${UBX2RINEX_VERSION}" ubx2rinex)
    if [[ -n "${TARGET_TRIPLE}" ]]; then
        INSTALL_ARGS+=(--target "${TARGET_TRIPLE}")
    fi
    cargo "${INSTALL_ARGS[@]}"
fi

if [[ ! -x "${UBX2RINEX_BIN}" ]]; then
    echo "ubx2rinex binary not found after install: ${UBX2RINEX_BIN}" >&2
    exit 1
fi

# 3) Assemble Debian package root filesystem.
STAGING_ROOT="${ROOT_DIR}/target/deb-staging"
PKG_DIR="${STAGING_ROOT}/${PACKAGE_NAME}_${APP_VERSION}_${DEB_ARCH}"
rm -rf "${PKG_DIR}"

install -d -m 0755 \
    "${PKG_DIR}/DEBIAN" \
    "${PKG_DIR}/usr/bin" \
    "${PKG_DIR}/usr/lib/gnss2tec-logger/bin" \
    "${PKG_DIR}/etc/gnss2tec-logger" \
    "${PKG_DIR}/lib/systemd/system"

install -m 0755 "${LOGGER_BIN}" "${PKG_DIR}/usr/bin/${PACKAGE_NAME}"
install -m 0755 "${UBX2RINEX_BIN}" "${PKG_DIR}/usr/lib/gnss2tec-logger/bin/ubx2rinex"
install -m 0644 "${ROOT_DIR}/packaging/config/ubx.dat" "${PKG_DIR}/etc/gnss2tec-logger/ubx.dat"
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
 compressed RINEX products (Hatanaka + gzip) using bundled ubx2rinex.
EOF

# Keep local receiver config changes across package upgrades/removal.
cat > "${PKG_DIR}/DEBIAN/conffiles" <<'EOF'
/etc/gnss2tec-logger/ubx.dat
EOF

# 4) Build the final .deb artifact.
mkdir -p "${OUT_DIR}"
DEB_PATH="${OUT_DIR}/${PACKAGE_NAME}_${APP_VERSION}_${DEB_ARCH}.deb"
dpkg-deb --root-owner-group --build "${PKG_DIR}" "${DEB_PATH}"

popd >/dev/null
echo "Built package: ${DEB_PATH}"
