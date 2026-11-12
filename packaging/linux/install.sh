#!/usr/bin/env bash
# Install wholeseek for the current user (no sudo):
#   ~/.local/bin/wholeseek
#   ~/.local/share/applications/wholeseek.desktop
#   ~/.local/share/icons/hicolor/<size>/apps/wholeseek.{png,svg}
# Usage: ./install.sh [--uninstall]
# Honours XDG_DATA_HOME; PREFIX overrides ~/.local for the binary.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
prefix=${PREFIX:-$HOME/.local}
bin_dir="$prefix/bin"
data_dir=${XDG_DATA_HOME:-$HOME/.local/share}
app_dir="$data_dir/applications"
icon_root="$data_dir/icons/hicolor"

refresh() {
  if command -v update-desktop-database >/dev/null; then
    update-desktop-database -q "$app_dir" 2>/dev/null || true
  fi
  if command -v gtk-update-icon-cache >/dev/null && [ -f "$icon_root/index.theme" ]; then
    gtk-update-icon-cache -q -t -f "$icon_root" 2>/dev/null || true
  fi
  if command -v xdg-desktop-menu >/dev/null; then
    xdg-desktop-menu forceupdate 2>/dev/null || true
  fi
}

case "${1:-}" in
  --uninstall)
    rm -f "$bin_dir/wholeseek" "$app_dir/wholeseek.desktop"
    for f in "$icon_root"/*/apps/wholeseek.png "$icon_root"/*/apps/wholeseek.svg; do
      [ -e "$f" ] && rm -f "$f"
    done
    refresh
    echo "wholeseek uninstalled (settings, cache and downloads were left alone)"
    exit 0
    ;;
  "") ;;
  -h | --help)
    sed -n '2,8p' "$0" | sed 's/^# \{0,1\}//'
    exit 0
    ;;
  *)
    echo "unknown option: $1 (use --uninstall or --help)" >&2
    exit 2
    ;;
esac

install -D -m 755 "$here/wholeseek" "$bin_dir/wholeseek"
# Absolute Exec= so the launcher works even when ~/.local/bin is not on PATH.
mkdir -p "$app_dir"
sed "s|^Exec=.*|Exec=$bin_dir/wholeseek|" "$here/wholeseek.desktop" >"$app_dir/wholeseek.desktop"
chmod 644 "$app_dir/wholeseek.desktop"
(cd "$here/icons/hicolor" && find . -type f) | while read -r f; do
  install -D -m 644 "$here/icons/hicolor/$f" "$icon_root/$f"
done
refresh

echo "wholeseek installed to $bin_dir/wholeseek"
case ":$PATH:" in
  *":$bin_dir:"*) ;;
  *) echo "note: $bin_dir is not on your PATH; the app menu entry works anyway" ;;
esac
