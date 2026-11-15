#!/usr/bin/env bash
# yd installer for Linux / macOS
#   curl -fsSL https://raw.githubusercontent.com/Slaviaaa2/yd/master/install.sh | bash
# Env:
#   YD_INSTALL_DIR  install directory (default: ~/.local/bin)
#   YD_VERSION      release tag, e.g. v1.0.0 (default: latest)
#   YD_MENU         yes / no: add an application menu entry (Linux) without asking
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

# Application menu entry (Linux desktop). `curl | bash` has the script on stdin,
# so the question is read from /dev/tty; without a terminal it is skipped unless YD_MENU=yes.
add_menu_entry() {
  local apps="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
  local file="$apps/yd.desktop"
  mkdir -p "$apps"
  cat >"$file" <<EOF
[Desktop Entry]
Type=Application
Name=yd
GenericName=Video & Audio Downloader
Comment=Fast yt-dlp front-end: paste a URL to download video or audio
Exec="$dir/yd"
Icon=folder-download
Terminal=true
Categories=Network;FileTransfer;
Keywords=youtube;yt-dlp;download;video;audio;mp3;
EOF
  command -v update-desktop-database >/dev/null && update-desktop-database "$apps" 2>/dev/null || true
  printf '\033[32m✔\033[0m Added to the application menu\n  └ %s\n' "$file"
}

if [ "$os" = linux ]; then
  menu="${YD_MENU:-}"
  if [ -z "$menu" ]; then
    if [ -r /dev/tty ] && { : </dev/tty; } 2>/dev/null; then
      printf 'Add yd to the application menu? [Y/n] ' >/dev/tty
      read -r answer </dev/tty || answer=""
      case "$answer" in [nN]*) menu=no ;; *) menu=yes ;; esac
    else
      menu=no
    fi
  fi
  case "$menu" in [yY]*|1|true) add_menu_entry ;; esac
fi

case ":$PATH:" in
  *":$dir:"*) ;;
  *)
    printf '\033[33m!\033[0m %s is not in PATH. Add this to your shell rc:\n' "$dir"
    printf '    export PATH="%s:$PATH"\n' "$dir"
    ;;
esac
