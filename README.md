<p align="center"><img src="assets/wholeseek-256.png" width="96" alt="wholeseek"></p>

# wholeseek

A minimal native desktop client for [Soulseek](https://www.slsknet.org/), and a music player for your own library.
Search, browse, press ▶ to hear a track instantly, and press ↓ only if you want to keep it.

- **Play before you download.** ▶ streams into a cache while the file arrives; nothing lands in
  your downloads folder unless you press ↓ Keep. Silent sources are skipped automatically.
- **Any format.** Filter by FLAC, MP3, WAV or other; results are grouped by user and folder with length, size and quality, best-available sources first.
- **Tabs.** Several searches and users open at once; click any username to browse their shares.
- **Album playback.** Play a folder in order with the next track prefetched.
- **Your library.** Albums, songs, instant search and stars for your own music folders, played
  right where the files are: nothing is copied, and nothing is added to your shares.
- **A good network citizen.** Logs in, shares your folders, serves uploads, maps its port.

Runs on Linux (x86_64), macOS (Apple Silicon) and Windows (x86_64, Windows 10+); downloads are
on the [website](https://wholeseek.utilitygods.com) and the
[releases page](https://github.com/Utility-Gods/wholeseek/releases). Native GPU UI
([rune](https://github.com/anomalyco/rune)), protocol by
[soulseek-rs](https://github.com/michel/soulseek-rs), audio by symphonia + cpal.

## Command line for agents (Linux and macOS)

`wholeseek search | browse | download | transfers | retry | status | cancel`, with `--json`.
That is the whole surface: no chat, settings, shares, uploads, library or playback. It uses the
login saved by the app and one Soulseek session at a time (the app, or a background helper that
starts on demand). See [docs/CLI.md](docs/CLI.md).

## Platforms

| | Download | First launch | Media keys |
|---|---|---|---|
| Linux x86_64 | `.tar.gz` (`./install.sh`) | — | MPRIS |
| macOS Apple Silicon | `.dmg` | System Settings → Privacy & Security → Open Anyway | Now Playing |
| Windows x86_64 | `.zip` (run `wholeseek.exe`) | SmartScreen: More info → Run anyway | not yet |

The Mac and Windows builds are not signed, which is why each asks once on first launch.

## Build

Needs Rust (the toolchain is pinned in `rust-toolchain.toml`) and ALSA headers
(`sudo apt install libasound2-dev`).

```sh
cargo run --release -p wholeseek          # the app
cargo run -p wholeseek -- --demo          # fake peers, no network, no audio
cargo test --workspace
```

Your password is kept in the system keyring; settings live in `~/.config/wholeseek/`.

### macOS (Apple Silicon, experimental)

Build a DMG on an M-series Mac (needs `xcode-select --install` and [rustup](https://rustup.rs)):

```sh
packaging/macos/build-dmg.sh
```

The DMG lands in `target/macos-dist/`. It is unsigned, so open it the first time with
right-click → Open.

### Windows (x86_64)

With [rustup](https://rustup.rs) and the Visual Studio C++ build tools (MSVC + Windows SDK),
`cargo run --release -p wholeseek` works as above. The release zip comes from Git Bash:

```sh
packaging/windows/build-zip.sh
```

It lands in `target/windows-dist/`. The SDK's `rc.exe` embeds the icon and version info.
Settings live in `%APPDATA%\wholeseek\`, the password in Windows Credential Manager,
downloads go to `Downloads\wholeseek`.

## Layout

| Path | What |
|---|---|
| `app/wholeseek` | the desktop app (UI) |
| `crates/wholeseek-core` | Soulseek session, search, browse, transfers, shares, config |
| `crates/wholeseek-audio` | streaming player (FLAC, MP3, WAV, OGG, AAC) that plays a file while it downloads |
| `docs/` | MVP definition and core API |

## License

MIT

## Third-party notices

Icons: a subset of [Lucide](https://lucide.dev) 0.577.0, vendored as SVG path data in
`app/wholeseek/src/view/icons.rs` (full notice there).

> ISC License
>
> Copyright (c) for portions of Lucide are held by Cole Bemis 2013-2026 as part of Feather
> (MIT). All other copyright (c) for Lucide are held by Lucide Contributors 2026.
>
> Permission to use, copy, modify, and/or distribute this software for any purpose with or
> without fee is hereby granted, provided that the above copyright notice and this permission
> notice appear in all copies.
>
> THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH REGARD TO THIS
> SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE
> AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
> WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT,
> NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR
> PERFORMANCE OF THIS SOFTWARE.
