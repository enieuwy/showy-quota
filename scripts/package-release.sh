#!/usr/bin/env bash
# Build the installable release tarball tree for one platform target.

set -euo pipefail

usage() {
    cat >&2 <<'USAGE'
Usage: VERSION=<version> TARGET=<target> RENDER_BIN=<path> OUT_DIR=<dir> scripts/package-release.sh

Creates showy-quota-<version>-<target>.tar.gz and a .sha256 sidecar in OUT_DIR.
VERSION may include a leading v; it is stripped for the archive name/root.
USAGE
}

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="${REPO_ROOT:-$(cd -- "${SCRIPT_DIR}/.." && pwd -P)}"
VERSION="${VERSION:-}"
TARGET="${TARGET:-}"
OUT_DIR="${OUT_DIR:-${REPO_ROOT}/dist}"
RENDER_BIN="${RENDER_BIN:-${REPO_ROOT}/target/release/showy-quota-render}"

if [[ -z "${VERSION}" || -z "${TARGET}" ]]; then
    usage
    exit 2
fi

VERSION="${VERSION#v}"
if [[ -z "${VERSION}" ]]; then
    printf 'showy-quota: VERSION must not be empty after stripping leading v\n' >&2
    exit 2
fi
if [[ ! "${VERSION}" =~ ^[A-Za-z0-9._+-]+$ ]]; then
    printf 'showy-quota: invalid VERSION for archive name: %s\n' "${VERSION}" >&2
    exit 2
fi
if [[ ! "${TARGET}" =~ ^[A-Za-z0-9._-]+$ ]]; then
    printf 'showy-quota: invalid TARGET for archive name: %s\n' "${TARGET}" >&2
    exit 2
fi
if [[ ! -x "${RENDER_BIN}" ]]; then
    printf 'showy-quota: render binary is missing or not executable: %s\n' "${RENDER_BIN}" >&2
    exit 1
fi

for path in bin lib adapters share showy-quota.tmux Makefile LICENSE README.md; do
    if [[ ! -e "${REPO_ROOT}/${path}" ]]; then
        printf 'showy-quota: required release path is missing: %s\n' "${path}" >&2
        exit 1
    fi
done

mkdir -p "${OUT_DIR}"
OUT_DIR="$(cd -- "${OUT_DIR}" && pwd -P)"

archive_root="showy-quota-${VERSION}"
artifact="showy-quota-${VERSION}-${TARGET}.tar.gz"
work_dir="${OUT_DIR}/.package-${TARGET}"
stage_root="${work_dir}/${archive_root}"

rm -rf "${work_dir}"
mkdir -p "${stage_root}"

for path in bin lib adapters share; do
    cp -R "${REPO_ROOT}/${path}" "${stage_root}/"
done
for path in showy-quota.tmux Makefile LICENSE README.md; do
    cp "${REPO_ROOT}/${path}" "${stage_root}/"
done

cp "${RENDER_BIN}" "${stage_root}/bin/showy-quota-render"
chmod +x "${stage_root}"/bin/showy-quota* "${stage_root}/bin/showy-quota-render"

rm -f "${OUT_DIR}/${artifact}" "${OUT_DIR}/${artifact}.sha256"
(
    cd -- "${work_dir}"
    tar -czf "${OUT_DIR}/${artifact}" "${archive_root}"
)

(
    cd -- "${OUT_DIR}"
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "${artifact}" > "${artifact}.sha256"
    else
        shasum -a 256 "${artifact}" > "${artifact}.sha256"
    fi
)

rm -rf "${work_dir}"
printf '%s\n' "${OUT_DIR}/${artifact}"
printf '%s\n' "${OUT_DIR}/${artifact}.sha256"
