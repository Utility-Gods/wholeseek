# wholeseek — agent instructions

- Scope is fixed by docs/MVP.md. Do not add features outside it.
- Workspace: crates/wholeseek-core (protocol/session/state), crates/wholeseek-audio (playback), app/wholeseek (Rune UI).
- Pinned deps live in the root Cargo.toml [workspace.dependencies]; do not bump without the Tech Lead.
- Every checkpoint needs proof: test output, headless PNG, or audio/log evidence saved under proof/<checkpoint>/ (local only, gitignored: logs contain machine paths).
- Never use real Soulseek credentials in code, tests or logs. Live-network tests read WHOLESEEK_USER / WHOLESEEK_PASS from env and are #[ignore] by default.
- Playing never writes to the downloads folder; it uses the cache dir.
- Before handing off: cargo fmt, cargo clippy -p <crate>, cargo test -p <crate>.
- Git: work on branch `staging`. Commit only your own finished, green files with `git commit -m "<area>: <what>" -- <paths>`, identity Turable <turable@utilitygods.com>, then `git push origin staging`. Never commit a file whose dependencies are uncommitted (staging must always build). Shared files (app.rs, tests.rs, main.rs, backend.rs, demo.rs) are committed by the Tech Lead. Forbidden: checkout/switch, reset, stash, rebase, merge, restore, clean, amend, force-push, tags, pushing main. `main` holds released code only; the Tech Lead promotes staging → main and tags releases.
