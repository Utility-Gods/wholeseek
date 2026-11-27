# wholeseek CLI (agent reference)

`wholeseek` is primarily a desktop app. This document covers the subset of
its command line meant for a scripted or AI agent: searching, browsing a
remote user, downloading, and managing transfers — no UI required.

Everything here is **Linux and macOS**. There is no Windows CLI surface.

## Why this exists: one Soulseek login

Soulseek only allows one active session per account at a time. wholeseek
therefore has exactly one local session owner per machine: either the
desktop app (while it's open) or a short-lived background **daemon** that
starts automatically the first time you run a CLI command and the app isn't
already running. You never start or address the daemon directly — every
command below connects to whichever one currently owns the session, waiting
briefly if one is still starting. If the daemon is idle (no commands, no
active transfers) for a while, it exits on its own; the next command starts
a fresh one. If the desktop app is already running, the CLI talks to it
instead and no daemon is spawned.

There is no separate CLI login. If neither the app nor a daemon holds saved
credentials yet, commands fail with exit code 3 ("not connected") — open the
desktop app once to sign in and save your Soulseek credentials; the CLI
reuses them from then on.

## Commands

All commands accept `--json` to print machine-readable output to stdout
instead of the default compact text. Errors (any non-zero exit) always go
to stderr in text mode, or to **stdout** as a JSON object in `--json` mode
— never both. Add `--help` to any command (or run `wholeseek --help`) for
a one-line usage reminder.

### `search`

```
wholeseek search "<query>" [--flac|--mp3|--wav|--lossless] [--min-bitrate N] [--limit N] [--wait SECS]
```

Searches the Soulseek network and waits (default up to 30s, `--wait` 1–60)
for results, then returns the best ones found so far, ranked by
availability (free upload slot, queue position, speed) — not format.

- At most one of `--flac`, `--mp3`, `--wav`, `--lossless` (lossless also
  matches WAV/AIFF/ALAC/APE/WavPack).
- `--min-bitrate N`: drop files with a known bitrate below N kbps (files
  with no reported bitrate, e.g. lossless, are never dropped by this).
- `--limit N` (default 20, max 100): maximum number of result groups.

Results are grouped **per remote folder** (one user's album, typically),
not per file — see the JSON shape below. Each group has an `id` you pass to
`download`.

Text example:

```
$ wholeseek search "boards of canada" --flac --limit 5
3 results:
  1. [8f3.2] peerA  Music/Boards of Canada/Music Has the Right to Children  FLAC 16/44.1  11 files  412.3 MB  free slot  1.2 MB/s
  2. [8f3.7] peerB  BoC/MHTRTC (1998)  FLAC 16/44.1  11 files  409.1 MB  queued #2  340 KB/s
  3. [8f3.9] peerC  music/boc/1998  mixed  12 files  430.0 MB  no free slot  0 KB/s
```

JSON (`--json`), the `SearchData` payload:

```json
{
  "groups": [
    {
      "id": "8f3.2",
      "user": "peerA",
      "folder": "Music\\Boards of Canada\\Music Has the Right to Children",
      "files": [
        {
          "path": "Music\\Boards of Canada\\...\\01 Wildlife Analysis.flac",
          "size_bytes": 12345678,
          "format": "flac",
          "bitrate_kbps": null,
          "bit_depth": 16,
          "sample_rate_hz": 44100,
          "duration_secs": 115,
          "vbr": null
        }
      ],
      "format": "flac",
      "bitrate_kbps": null,
      "bit_depth": 16,
      "sample_rate_hz": 44100,
      "size_bytes": 432198765,
      "file_count": 11,
      "free_slot": true,
      "queue_position": null,
      "speed_bytes_per_sec": 1258291
    }
  ]
}
```

Note: remote paths use Soulseek's native `\` separator even on Linux/macOS
(the network is Windows-path-native); pass them through unchanged to
`download`/`browse`.

**Search etiquette (enforced by the daemon, not just a suggestion):**
searches are rate-limited to a minimum of **15 seconds apart** and **40 per
hour**. A search issued too soon fails with exit code 5 and a
`retry_after_secs` telling you how long to wait — space out automated
search loops accordingly, don't retry in a tight loop.

### `browse`

```
wholeseek browse <user> [<folder>]
```

Lists a remote user's shared folders. With no `<folder>`, lists every
shared folder and its files (can be large for a big share). With
`<folder>` (the exact remote path from a `search` result's `folder`
field, or a `BrowseFolder.path`), lists just that one folder. Browsing can
take up to ~65 seconds if the peer is slow to respond; fails with exit code
4 if the folder doesn't exist in the listing.

```
$ wholeseek browse peerA "Music\Boards of Canada\Music Has the Right to Children"
peerA:
  Music/Boards of Canada/Music Has the Right to Children/
    01 Wildlife Analysis.flac  FLAC 16/44.1  11.2 MB
    02 An Eagle in Your Mind.flac  FLAC 16/44.1  18.4 MB
```

JSON, the `BrowseData` payload — same `FileData` shape as `search`:

```json
{
  "user": "peerA",
  "folders": [
    { "path": "Music\\Boards of Canada\\...", "files": [ { "path": "...", "size_bytes": 1, "format": "flac", "bitrate_kbps": null, "bit_depth": 16, "sample_rate_hz": 44100, "duration_secs": 1, "vbr": null } ] }
  ]
}
```

### `download`

```
wholeseek download <result-id> [--to DIR] [--wait]
wholeseek download <user> <path> [--to DIR] [--wait]
```

Downloads an entire result group (by the `id` from `search`, downloading
the whole folder) or a single remote file (by `<user>` and its exact
Soulseek path — from `search`/`browse`, or a folder path from `browse` to
download everything in it). Without `--wait`, returns immediately once the
transfer(s) are queued, each with a `transfers`-shaped `id` to poll later.
With `--wait`, blocks (up to ~55 minutes) until every file in the request
finishes, then returns their final state; a failed or cancelled transfer
under `--wait` is reported as exit code 6.

`--to DIR` must be an **absolute path**; otherwise uses the app's
configured download folder.

A `<result-id>` expires after a while (same lifetime as the search that
produced it) — exit code 4 if it's gone; search again.

```
$ wholeseek download 8f3.2 --to /home/me/Music --json
{"transfers":[{"id":"c-1234-56","state":"queued","bytes":0,"total_bytes":12345678,"speed_bytes_per_sec":0.0,"queue_position":null,"user":"peerA","path":"Music\\...\\01 Wildlife Analysis.flac","error":null,"origin":"cli"}]}
```

### `transfers`

```
wholeseek transfers [<id>]
```

Lists all CLI-started transfers (downloads), or just one by `<id>`. States:
`queued`, `downloading`, `done`, `failed`, `cancelled`. (Transfers started
from the desktop app are not shown or controllable here.)

```
$ wholeseek transfers
c-1234-56  downloading  120.0 MB/430.0 MB (27%)  850 KB/s  peerA  Music\...\01 Wildlife Analysis.flac
```

### `retry`

```
wholeseek retry <id> --other-source
```

(`--other-source` is required and is the only supported form — there is no
plain retry.) Cancels the transfer if still active, picks a different
source for the **same file** from recent search results (one not already
tried), and starts a new transfer with a new `id`. Fails with exit code 4
if no alternate source has been seen recently — search again first. Fails
with exit code 7 if `<id>` wasn't started by the CLI.

```
$ wholeseek retry c-1234-56 --other-source --json
{"transfers":[{"id":"c-1234-78","state":"queued", ...}]}
```

### `status`

```
wholeseek status
```

Reports whether Soulseek is connected, whether you're talking to the
`app` or a background `daemon`, and (daemon only) its configured idle
timeout in seconds.

```
$ wholeseek status --json
{"connected":true,"mode":"daemon","idle_timeout_secs":600}
```

### `cancel`

```
wholeseek cancel <id>
```

Cancels an active CLI-started download. Exit code 4 if `<id>` is unknown,
7 if it belongs to the desktop app instead.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | success |
| 1 | internal error (including "busy": too many requests in flight) |
| 2 | usage error: bad command, flag or argument — nothing was sent to the session |
| 3 | not connected: Soulseek session isn't up (e.g. no saved credentials yet) |
| 4 | unknown id (result, transfer, or remote folder not found) |
| 5 | rate limited (search); `retry_after_secs` in the error says how long to wait |
| 6 | transfer failed or was cancelled while a command was waiting on it |
| 7 | rejected (e.g. trying to cancel/retry a transfer the CLI didn't start) |

A **usage error (exit 2)** never contacts the Soulseek session: the command
line itself was invalid, nothing was sent anywhere.

## JSON error shape

Any failing command with `--json` prints exactly this to **stdout** (not
stderr) and exits with the mapped code:

```json
{"error":{"code":"rate_limited","message":"search rate limit reached","retry_after_secs":12}}
```

`retry_after_secs` is present only for rate-limit errors. `code` is one of:
`internal`, `bad_request`, `not_connected`, `not_found`, `rate_limited`,
`transfer_failed`, `forbidden`, `busy`.

## Recipe: find and download an album

```sh
# 1. Search, preferring FLAC.
wholeseek search "boards of canada music has the right" --flac --json > results.json

# 2. Pick a group (e.g. the one with a free slot and best speed — already
#    the top-ranked one) and read its id.
id=$(jq -r '.groups[0].id' results.json)

# 3. Download the whole folder, waiting for it to finish.
wholeseek download "$id" --to ~/Music --wait --json > download.json

# 4. If it failed (exit code 6), or you'd rather try a different peer,
#    get the transfer id and switch sources.
tid=$(jq -r '.transfers[0].id' download.json)
wholeseek retry "$tid" --other-source --json > retry.json
new_tid=$(jq -r '.transfers[0].id' retry.json)

# 5. retry doesn't block; poll until it's done.
until [ "$(wholeseek transfers "$new_tid" --json | jq -r '.transfers[0].state')" != "downloading" ] && \
      [ "$(wholeseek transfers "$new_tid" --json | jq -r '.transfers[0].state')" != "queued" ]; do
  sleep 2
done
```

If `retry` exits 4, there's no other known source yet: re-run `search` to
refresh the pool of known peers for that file, then retry.

## What it can't do

- No chat, rooms, private messages, buddies or wishlists.
- No browsing/editing your own shares, uploads, library, or settings.
- No playback — this CLI only downloads, it never streams/plays audio.
- No login/logout or credential management from the CLI; sign in once via
  the desktop app.
- No Windows support (Linux and macOS only).
- No commands beyond the seven above — anything else is a usage error
  (exit code 2), by design, not a missing feature to work around.
