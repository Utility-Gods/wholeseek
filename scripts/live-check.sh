#!/usr/bin/env bash
# Live Soulseek check for wholeseek (checkpoint 1).
# Prompts for credentials in YOUR terminal (password is not echoed, not saved,
# not written to the log). Uses temporary download/cache dirs, never your real ones.
# Log out of Nicotine+ first: Soulseek disconnects the older session.
set -euo pipefail
cd "$(dirname "$0")/.."

read -rp  "Soulseek username: " WHOLESEEK_USER
read -rsp "Soulseek password: " WHOLESEEK_PASS; echo
read -rp  "Search query [aphex twin]: " WHOLESEEK_QUERY
WHOLESEEK_QUERY=${WHOLESEEK_QUERY:-aphex twin}
export WHOLESEEK_USER WHOLESEEK_PASS WHOLESEEK_QUERY

mkdir -p proof/cp1/live
log=proof/cp1/live/core-live.log
{
  echo "# $(date -Is) $(rustc --version)"
  echo "# cmd: cargo test -p wholeseek-core --test live -- --ignored --nocapture"
  echo "# query: $WHOLESEEK_QUERY"
} > "$log"

set +e
cargo test -p wholeseek-core --test live -- --ignored --nocapture 2>&1 | tee -a "$log"
status=${PIPESTATUS[0]}
set -e
unset WHOLESEEK_PASS

# Belt and braces: make sure the password never reached the log.
echo "# exit: $status" >> "$log"
echo
echo "Done (exit $status). Log: $log"
