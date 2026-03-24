#!/usr/bin/env bash
set -euo pipefail

BINARY_NAME="${BINARY_NAME:-gstackqlite-hypervisor}"
TARGET="${TARGET:?TARGET environment variable is required}"
RAW_VERSION="${VERSION:?VERSION environment variable is required}"
VERSION="${RAW_VERSION#v}"
DIST_DIR="${DIST_DIR:-dist}"

mkdir -p "${DIST_DIR}"

cargo build --release --locked --target "${TARGET}"

archive_root="${BINARY_NAME}-${VERSION}-${TARGET}"
stage_dir="$(mktemp -d)"
package_dir="${stage_dir}/${archive_root}"
mkdir -p "${package_dir}"

cp "target/${TARGET}/release/${BINARY_NAME}" "${package_dir}/"
cp README.md "${package_dir}/"
cp scripts/install.sh "${package_dir}/install.sh"
cp scripts/install.ps1 "${package_dir}/install.ps1"
if [[ -f LICENSE ]]; then
  cp LICENSE "${package_dir}/"
fi

tar -C "${stage_dir}" -czf "${DIST_DIR}/${archive_root}.tar.gz" "${archive_root}"
rm -rf "${stage_dir}"
