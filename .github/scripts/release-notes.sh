#!/usr/bin/env bash
# Print the CHANGELOG.md section for a version ("0.1.4" or "v0.1.4"), without its heading.
# Fails if the section is missing or empty, so a release can't ship without notes.
set -euo pipefail
version="${1#v}"
notes=$(awk -v v="$version" '
  /^## / { if (inside) exit; inside = ($2 == v); next }
  inside { print }
' "${2:-CHANGELOG.md}" | sed -e '/./,$!d' | sed -e ':a' -e '/^\n*$/{$d;N;ba' -e '}')
if [ -z "$notes" ]; then
  echo "CHANGELOG.md has no notes for $version" >&2
  exit 1
fi
printf '%s\n' "$notes"
