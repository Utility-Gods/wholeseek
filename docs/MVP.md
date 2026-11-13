# wholeseek — MVP

Status: **final**. License: **MIT**. Platform: **Linux** (Rune is cross-platform; others later).

## Product

A minimal native desktop Soulseek client for music: **search, browse a user, play instantly, download if you like it.** It is a proper network citizen (shares and uploads), but that stays out of the main view.

## Core rule: playing ≠ downloading

- **▶ Play** streams the file into a temporary cache and starts playback as soon as the first chunk is decodable. Nothing lands in the downloads folder.
- **↓ Download** saves to the downloads folder. If the track is already fully cached from playing, it is moved instead of fetched again.
- Cache is size-capped and cleared automatically.

## In scope

| Area | Behaviour |
|---|---|
| Login | First launch: Soulseek username/password (OS keyring). Auto-reconnect. |
| Shares | Choose folder(s); index on start and on rescan. Peers can search/browse/download them. |
| Uploads | Slot cap and speed limit; queue handled by the library. |
| Search | Network search; audio-only by default; format filters (FLAC · MP3 · WAV · Other, none = all); ordered by availability (free slot, queue, speed), not format; free slot and speed visible. |
| Browse user | Click a username → their folder tree in place; Back returns to results. |
| Player | Small bottom strip: play/pause, seek (within buffered range), volume, track/user. One track at a time. |
| Download | File or whole folder; keeps folder name. |
| Transfers | Compact expandable panel: downloads + uploads, progress, cancel, retry. |
| Library | Your own music, played in place (never copied): choose folders (default: Music + Downloads); background incremental scan; Albums and Songs views; instant search; star songs/albums; play song/album as queue. |
| Settings | Account, shares, download folder, library folders, listen port, slots/speed limit. |

## Out of scope

Chat/rooms, private messages, wishlist, buddies, playlists, tag editing, artist pages, themes, Windows builds.

## Layout

```
┌───────────────────────────────────────────────────────┐
│ [ Search music…                        ]   ⇅ 2  ⚙     │
├───────────────────────────────────────────────────────┤
│ ← Back  alice / Music / Album (2019)                   │
│ 01 Track.flac      16/44.1  28 MB              ▶  ↓    │
│ 02 Track.flac      16/44.1  31 MB              ▶  ↓    │
├───────────────────────────────────────────────────────┤
│ ❚❚  01 Track — alice   ━━━━━━──────  1:24 / 4:10  🔊  │
└───────────────────────────────────────────────────────┘
⇅ transfers · ⚙ settings · first launch: login → shares → connect
```

## Technical decisions

| Concern | Choice | Why |
|---|---|---|
| Protocol | [`soulseek-rs-lib`](https://github.com/michel/soulseek-rs) (MIT, Rust, pinned rev) | Full modern protocol: search, browse, shares, upload queue, resumable downloads, firewalled peers, UPnP/NAT-PMP. Zero deps. Downloads stream to `.part` with progress events — enough for play-while-downloading. |
| UI | [Rune](https://github.com/anomalyco/rune) (pinned `dev` rev) | Native GPU Rust UI, headless render/test. Pre-release accepted. |
| Audio | symphonia (FLAC/MP3/OGG/M4A decode) + cpal (output) | Pure Rust. |
| Secrets | `keyring` crate | Password not stored in plain config. |
| Config | TOML in `~/.config/wholeseek/` | Simple. |

Rejected: slskd sidecar (.NET runtime, second process, AGPL); writing our own protocol (unnecessary given soulseek-rs-lib).

## Architecture

```
wholeseek/
├─ crates/wholeseek-core   session wrapper over soulseek-rs-lib, search/browse/transfer state, config, cache
├─ crates/wholeseek-audio  play a growing file: wait-for-bytes reader → symphonia → cpal
└─ app/wholeseek           Rune UI
```

The UI never blocks: core runs the client on its own thread(s) and sends events to the UI state.

## Milestones

0. **Spikes** — Rune window on Linux; soulseek-rs-lib login + search (needs your account, with permission); symphonia plays a growing `.part` FLAC.
1. **Core** — session, search, browse, downloads/cache, shares, uploads, config/keyring.
2. **Audio** — growing-file player with buffered-range seek.
3. **UI** — onboarding, search, browse, player strip, transfers, settings.
4. **Polish** — reconnect, error states, headless UI tests, AppImage.

## Done when

- Fresh install → log in → pick share folder → connected, share counts shown.
- Search → results in seconds → ▶ a track starts within a few seconds of transfer start; nothing appears in downloads.
- ↓ on the same track saves it (reusing the cache when complete).
- Open a user → browse → ▶ and ↓ work there too.
- Another client (e.g. Nicotine+) can browse and download from wholeseek.
