#!/usr/bin/env bash
# yd installer for Linux / macOS
#   curl -fsSL https://raw.githubusercontent.com/Slaviaaa2/yd/master/install.sh | bash
# Env:
#   YD_INSTALL_DIR  install directory (default: ~/.local/bin)
#   YD_VERSION      release tag, e.g. v1.0.0 (default: latest)
set -euo pipefail

repo="Slaviaaa2/yd"
dir="${YD_INSTALL_DIR:-$HOME/.local/bin}"
version="${YD_VERSION:-latest}"

say() { printf '\033[2m•\033[0m %s\n' "$*"; }
die() { printf '\033[31m✘\033[0m %s\n' "$*" >&2; exit 1; }

case "$(uname -s)" in
  Linux)  os=linux ;;
  Darwin) os=macos ;;
  *) die "unsupported OS: $(uname -s) (Windows: use install.ps1)" ;;
esac
case "$(uname -m)" in
  x86_64|amd64)  arch=x64 ;;
  aarch64|arm64) arch=arm64 ;;
  *) die "unsupported architecture: $(uname -m)" ;;
esac

asset="yd-${os}-${arch}.tar.gz"
if [ "$version" = latest ]; then
  url="https://github.com/${repo}/releases/latest/download/${asset}"
else
  url="https://github.com/${repo}/releases/download/${version}/${asset}"
fi

command -v curl >/dev/null || die "curl is required"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

say "Downloading ${asset} (${version})"
curl -fL --progress-bar "$url" -o "$tmp/$asset" || die "download failed: $url"
tar -xzf "$tmp/$asset" -C "$tmp"
bin="$(find "$tmp" -type f -name yd | head -n1)"
[ -n "$bin" ] || die "yd binary not found in archive"

mkdir -p "$dir"
install -m 755 "$bin" "$dir/yd"
printf '\033[32m✔\033[0m Installed %s\n' "$dir/yd"
"$dir/yd" --version 2>/dev/null | sed 's/^/  └ /' || true

case ":$PATH:" in
  *":$dir:"*) ;;
  *)
    printf '\033[33m!\033[0m %s is not in PATH. Add this to your shell rc:\n' "$dir"
    printf '    export PATH="%s:$PATH"\n' "$dir"
    ;;
esac
