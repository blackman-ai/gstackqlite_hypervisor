#!/usr/bin/env bash
set -euo pipefail

BINARY_NAME="gstackqlite-hypervisor"
DEFAULT_INSTALL_DIR="${HOME}/.local/bin"
INSTALL_DIR="${GSTACKQLITE_HYPERVISOR_INSTALL_DIR:-${GSTACK_HYPERVISOR_INSTALL_DIR:-${DEFAULT_INSTALL_DIR}}}"
VERSION="${GSTACKQLITE_HYPERVISOR_VERSION:-${GSTACK_HYPERVISOR_VERSION:-latest}}"
REPOSITORY="${GSTACKQLITE_HYPERVISOR_REPO:-${GSTACK_HYPERVISOR_REPO:-blackman-ai/gstackqlite_hypervisor}}"
AGENT_INSTALL="${GSTACKQLITE_HYPERVISOR_AGENT_INSTALL:-${GSTACK_HYPERVISOR_AGENT_INSTALL:-prompt}}"
UPDATE_PATH=1
SCRIPT_SOURCE="${BASH_SOURCE[0]-}"
SCRIPT_DIR=""
LOCAL_BINARY=""
if [[ -n "${SCRIPT_SOURCE}" && -f "${SCRIPT_SOURCE}" ]]; then
  SCRIPT_DIR="$(cd -- "$(dirname -- "${SCRIPT_SOURCE}")" && pwd -P)"
  LOCAL_BINARY="${SCRIPT_DIR}/${BINARY_NAME}"
fi

usage() {
  cat <<'EOF'
Install gstackqlite-hypervisor from a GitHub release.
If the script is next to a packaged binary, it installs that local copy instead.

Usage:
  install.sh [--repo owner/repo] [--version v0.0.4|latest] [--install-dir /path] [--no-path-update]

Environment:
  GSTACKQLITE_HYPERVISOR_REPO         GitHub repository slug, for example "owner/gstackqlite_hypervisor"
  GSTACKQLITE_HYPERVISOR_VERSION      Release tag to install, defaults to "latest"
  GSTACKQLITE_HYPERVISOR_INSTALL_DIR  Install directory, defaults to "$HOME/.local/bin"
  GSTACKQLITE_HYPERVISOR_AGENT_INSTALL Agent bootstrap choice: "claude", "codex", "both", "none", or "prompt"

Compatibility aliases:
  GSTACK_HYPERVISOR_REPO
  GSTACK_HYPERVISOR_VERSION
  GSTACK_HYPERVISOR_INSTALL_DIR
  GSTACK_HYPERVISOR_AGENT_INSTALL
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --repo)
      REPOSITORY="${2:?missing value for --repo}"
      shift 2
      ;;
    --version)
      VERSION="${2:?missing value for --version}"
      shift 2
      ;;
    --install-dir)
      INSTALL_DIR="${2:?missing value for --install-dir}"
      shift 2
      ;;
    --no-path-update)
      UPDATE_PATH=0
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

need_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Missing required command: $1" >&2
    exit 1
  fi
}

command_exists() {
  command -v "$1" >/dev/null 2>&1
}

need_cmd uname
need_cmd mktemp
need_cmd tar

if command -v curl >/dev/null 2>&1; then
  fetch() {
    curl -fsSL "$1" -o "$2"
  }
  fetch_text() {
    curl -fsSL "$1"
  }
elif command -v wget >/dev/null 2>&1; then
  fetch() {
    wget -qO "$2" "$1"
  }
  fetch_text() {
    wget -qO- "$1"
  }
else
  echo "Missing downloader: install curl or wget." >&2
  exit 1
fi

resolve_version() {
  if [[ "${VERSION}" != "latest" ]]; then
    echo "${VERSION}"
    return
  fi

  local latest_api tag
  latest_api="https://api.github.com/repos/${REPOSITORY}/releases/latest"
  tag="$(fetch_text "${latest_api}" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)"
  if [[ -z "${tag}" ]]; then
    echo "Failed to resolve latest release tag from ${latest_api}" >&2
    exit 1
  fi
  echo "${tag}"
}

sha256_check() {
  local file="$1"
  local expected="$2"
  if command -v shasum >/dev/null 2>&1; then
    local actual
    actual="$(shasum -a 256 "${file}" | awk '{print $1}')"
    [[ "${actual}" == "${expected}" ]]
    return
  fi
  if command -v sha256sum >/dev/null 2>&1; then
    local actual
    actual="$(sha256sum "${file}" | awk '{print $1}')"
    [[ "${actual}" == "${expected}" ]]
    return
  fi
  if command -v openssl >/dev/null 2>&1; then
    local actual
    actual="$(openssl dgst -sha256 "${file}" | awk '{print $NF}')"
    [[ "${actual}" == "${expected}" ]]
    return
  fi
  echo "Missing checksum tool: install shasum, sha256sum, or openssl." >&2
  exit 1
}

detect_target() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"

  case "${arch}" in
    x86_64|amd64) arch="x86_64" ;;
    arm64|aarch64) arch="aarch64" ;;
    *)
      echo "Unsupported architecture: ${arch}" >&2
      exit 1
      ;;
  esac

  case "${os}" in
    Darwin) echo "${arch}-apple-darwin" ;;
    Linux)
      if [[ "${arch}" != "x86_64" ]]; then
        echo "Linux ${arch} releases are not published yet." >&2
        exit 1
      fi
      echo "${arch}-unknown-linux-gnu"
      ;;
    *)
      echo "Unsupported operating system: ${os}" >&2
      exit 1
      ;;
  esac
}

append_path_line() {
  local file="$1"
  local line="$2"
  mkdir -p "$(dirname "${file}")"
  touch "${file}"
  if ! grep -Fqs "${line}" "${file}"; then
    printf '\n%s\n' "${line}" >> "${file}"
  fi
}

maybe_update_path() {
  if [[ "${UPDATE_PATH}" -ne 1 ]]; then
    return
  fi

  case ":${PATH}:" in
    *":${INSTALL_DIR}:"*) return ;;
  esac

  local shell_name shell_rc
  shell_name="$(basename "${SHELL:-}")"
  case "${shell_name}" in
    zsh)
      shell_rc="${ZDOTDIR:-${HOME}}/.zshrc"
      append_path_line "${shell_rc}" "export PATH=\"${INSTALL_DIR}:\$PATH\""
      echo "Added ${INSTALL_DIR} to PATH in ${shell_rc}"
      ;;
    bash)
      if [[ -f "${HOME}/.bash_profile" || "$(uname -s)" == "Darwin" ]]; then
        shell_rc="${HOME}/.bash_profile"
      else
        shell_rc="${HOME}/.bashrc"
      fi
      append_path_line "${shell_rc}" "export PATH=\"${INSTALL_DIR}:\$PATH\""
      echo "Added ${INSTALL_DIR} to PATH in ${shell_rc}"
      ;;
    fish)
      shell_rc="${XDG_CONFIG_HOME:-${HOME}/.config}/fish/config.fish"
      append_path_line "${shell_rc}" "fish_add_path -Ua \"${INSTALL_DIR}\""
      echo "Added ${INSTALL_DIR} to PATH in ${shell_rc}"
      ;;
    *)
      shell_rc="${HOME}/.profile"
      append_path_line "${shell_rc}" "export PATH=\"${INSTALL_DIR}:\$PATH\""
      echo "Added ${INSTALL_DIR} to PATH in ${shell_rc}"
      ;;
  esac
}

ensure_bun_in_path() {
  export BUN_INSTALL="${BUN_INSTALL:-${HOME}/.bun}"
  case ":${PATH}:" in
    *":${BUN_INSTALL}/bin:"*) ;;
    *) export PATH="${BUN_INSTALL}/bin:${PATH}" ;;
  esac
}

install_bun_if_needed() {
  if command_exists bun; then
    return
  fi

  echo "Bun was not found. Installing Bun..."
  fetch_text "https://bun.com/install" | bash
  ensure_bun_in_path

  if ! command_exists bun; then
    echo "Bun install completed, but 'bun' is still not available on PATH." >&2
    echo "Try opening a new shell and rerunning the installer." >&2
    exit 1
  fi
}

resolve_agent_selection() {
  local normalized
  normalized="$(printf '%s' "${AGENT_INSTALL}" | tr '[:upper:]' '[:lower:]' | tr -d '[:space:]')"
  case "${normalized}" in
    claude|codex|both|none)
      printf '%s\n' "${normalized}"
      return
      ;;
    ""|prompt)
      ;;
    *)
      echo "Unsupported GSTACKQLITE_HYPERVISOR_AGENT_INSTALL value: ${AGENT_INSTALL}" >&2
      exit 1
      ;;
  esac

  if [[ ! -t 0 ]]; then
    echo "Skipping Claude/Codex bootstrap because stdin is not interactive." >&2
    echo "Set GSTACKQLITE_HYPERVISOR_AGENT_INSTALL=claude|codex|both|none to override." >&2
    printf 'none\n'
    return
  fi

  while true; do
    printf 'Neither Claude nor Codex is installed. Install which agent(s)? [claude/codex/both/none] '
    read -r normalized
    normalized="$(printf '%s' "${normalized}" | tr '[:upper:]' '[:lower:]' | tr -d '[:space:]')"
    case "${normalized}" in
      claude|codex|both|none)
        printf '%s\n' "${normalized}"
        return
        ;;
    esac
    echo "Enter one of: claude, codex, both, none." >&2
  done
}

install_claude_if_needed() {
  if command_exists claude; then
    return
  fi
  ensure_bun_in_path
  echo "Installing Claude Code with Bun..."
  bun install --global @anthropic-ai/claude-code
}

install_codex_if_needed() {
  if command_exists codex; then
    return
  fi
  ensure_bun_in_path
  echo "Installing Codex CLI with Bun..."
  bun install --global @openai/codex
}

maybe_install_agents() {
  local has_claude=0 has_codex=0 selection
  command_exists claude && has_claude=1
  command_exists codex && has_codex=1
  if [[ "${has_claude}" -eq 1 || "${has_codex}" -eq 1 ]]; then
    return
  fi

  selection="$(resolve_agent_selection)"
  case "${selection}" in
    claude)
      install_claude_if_needed
      ;;
    codex)
      install_codex_if_needed
      ;;
    both)
      install_claude_if_needed
      install_codex_if_needed
      ;;
    none)
      echo "Skipping Claude/Codex bootstrap."
      ;;
  esac
}

resolved_version="$(resolve_version)"
target="$(detect_target)"
archive_name="${BINARY_NAME}-${resolved_version#v}-${target}.tar.gz"
temp_dir="$(mktemp -d)"
archive_path="${temp_dir}/${archive_name}"
checksums_path="${temp_dir}/SHA256SUMS"

install_binary() {
  local source_path="$1"
  mkdir -p "${INSTALL_DIR}"
  cp "${source_path}" "${INSTALL_DIR}/${BINARY_NAME}"
  chmod +x "${INSTALL_DIR}/${BINARY_NAME}"
  maybe_update_path
  install_bun_if_needed
  maybe_install_agents
  echo "Installed ${BINARY_NAME} to ${INSTALL_DIR}/${BINARY_NAME}"
  echo "Run '${BINARY_NAME} --help' after opening a new shell, or export PATH=\"${INSTALL_DIR}:\$PATH\" now."
}

if [[ -n "${LOCAL_BINARY}" && -f "${LOCAL_BINARY}" ]]; then
  echo "Installing ${BINARY_NAME} from local package..."
  install_binary "${LOCAL_BINARY}"
  exit 0
fi

if [[ "${resolved_version}" == "latest" ]]; then
  release_url="https://github.com/${REPOSITORY}/releases/latest/download"
else
  release_url="https://github.com/${REPOSITORY}/releases/download/${resolved_version}"
fi

echo "Downloading ${archive_name} from ${REPOSITORY}..."
fetch "${release_url}/${archive_name}" "${archive_path}"
fetch "${release_url}/SHA256SUMS" "${checksums_path}"

expected_sum="$(awk -v file="${archive_name}" '$2 == file { print $1 }' "${checksums_path}")"
if [[ -z "${expected_sum}" ]]; then
  echo "Checksum entry not found for ${archive_name}" >&2
  exit 1
fi

if ! sha256_check "${archive_path}" "${expected_sum}"; then
  echo "Checksum verification failed for ${archive_name}" >&2
  exit 1
fi

mkdir -p "${INSTALL_DIR}"
tar -xzf "${archive_path}" -C "${temp_dir}"
install_binary "${temp_dir}/${BINARY_NAME}-${resolved_version#v}-${target}/${BINARY_NAME}"

rm -rf "${temp_dir}"
