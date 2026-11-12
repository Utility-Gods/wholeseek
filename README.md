<p align="center"><img src="assets/wholeseek-256.png" width="96" alt="wholeseek"></p>

# wholeseek

A minimal native desktop client for [Soulseek](https://www.slsknet.org/), made for music.
Search, browse, press ▶ to hear a track instantly, and press ↓ only if you want to keep it.

- **Play before you download.** ▶ streams into a cache while the file arrives; nothing lands in
  your downloads folder unless you press ↓ Keep. Silent sources are skipped automatically.
- **Any format.** Filter by FLAC, MP3, WAV or other; results are grouped by user and folder with length, size and quality, best-available sources first.
- **Tabs.** Several searches and users open at once; click any username to browse their shares.
- **Album playback.** Play a folder in order with the next track prefetched.
- **A good network citizen.** Logs in, shares your folders, serves uploads, maps its port.

Linux only for now. Native GPU UI ([rune](https://github.com/anomalyco/rune)), protocol by
[soulseek-rs](https://github.com/michel/soulseek-rs), audio by symphonia + cpal.

## Build

Needs Rust (the toolchain is pinned in `rust-toolchain.toml`) and ALSA headers
(`sudo apt install libasound2-dev`).

```sh
cargo run --release -p wholeseek          # the app
cargo run -p wholeseek -- --demo          # fake peers, no network, no audio
cargo test --workspace
```

Your password is kept in the system keyring; settings live in `~/.config/wholeseek/`.

### macOS (experimental)

Build a DMG on your Mac (needs `xcode-select --install` and [rustup](https://rustup.rs)):

```sh
packaging/macos/build-dmg.sh               # this Mac's chip
UNIVERSAL=1 packaging/macos/build-dmg.sh   # Apple Silicon + Intel
```

The DMG lands in `target/macos-dist/`. It is unsigned, so open it the first time with
right-click → Open.

## Layout

| Path | What |
|---|---|
| `app/wholeseek` | the desktop app (UI) |
| `crates/wholeseek-core` | Soulseek session, search, browse, transfers, shares, config |
| `crates/wholeseek-audio` | streaming player (FLAC, MP3, WAV, OGG, AAC) that plays a file while it downloads |
| `docs/` | MVP definition and core API |

## License

MIT
