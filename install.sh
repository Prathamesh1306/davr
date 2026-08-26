#!/usr/bin/env bash
set -euo pipefail

# DAVR Installer Script
# Installs the `davr` binary to ~/.local/bin or ~/.cargo/bin

INSTALL_DIR="${HOME}/.local/bin"
if [[ -d "${HOME}/.cargo/bin" ]]; then
    INSTALL_DIR="${HOME}/.cargo/bin"
fi

mkdir -p "${INSTALL_DIR}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "==> Building and installing DAVR..."

if command -v cargo >/dev/null 2>&1; then
    cargo install --path "${SCRIPT_DIR}/crates/davr-cli" --root "${HOME}/.local" --force 2>/dev/null || \
    cargo build --release --manifest-path "${SCRIPT_DIR}/Cargo.toml" -p davr-cli
    
    if [[ -f "${SCRIPT_DIR}/target/release/davr" ]]; then
        cp "${SCRIPT_DIR}/target/release/davr" "${INSTALL_DIR}/davr"
    elif [[ -f "${SCRIPT_DIR}/target/debug/davr" ]]; then
        cp "${SCRIPT_DIR}/target/debug/davr" "${INSTALL_DIR}/davr"
    fi
else
    echo "Error: Rust toolchain (cargo) is required to build from source."
    echo "Install Rust via: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
    exit 1
fi

chmod +x "${INSTALL_DIR}/davr"

echo "✔ Successfully installed davr to ${INSTALL_DIR}/davr"
echo ""
echo "Verify installation by running:"
echo "  davr --version"
echo "  davr --help"
