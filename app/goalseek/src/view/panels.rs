//! ⇅ Transfers and ⚙ Settings, docked to the right of the content.

use goalseek_core::{TransferKind, UploadState};
use rune::tok::{Theme, type_};
use rune::{El, ScrollSpec, col, el, row, text};

use super::{
    accent, button, field_box, green, hairline, icon_button, icons, input, progress, red, section,
    tnum,
};
use crate::app::{App, Conn, Panel, TState, Transfer};
use crate::fmt;

const W: f32 = 380.0;

fn panel(key: &str, title: &str, body: El, t: &Theme) -> El {
    let close = icon_button("panel-close", icons::close(10.0, t.text_secondary), t)
        .label("Close")
        .on_press(|s: &mut App| s.panel = Panel::None);
    col()
        .key(key.to_string())
        .w(W)
        .h_full()
        .min_h(0.0)
        .fill(t.bg)
        .child(
            row()
                .h(44.0)
                .w_full()
                .pl(16.0)
                .pr(10.0)
                .center_y()
                .justify_between()
                .child(text(title.to_string()).font(type_::TITLE).color(t.text))
                .child(close),
        )
        .child(hairline(t))
        .child(
            body.scroll(ScrollSpec {
                stick_bottom: false,
            })
            .shrink()
            .min_h(0.0),
        )
}

pub fn transfers(s: &App, t: &Theme) -> El {
    let downloads: Vec<&Transfer> = s
        .transfers
        .iter()
        .filter(|x| x.kind == TransferKind::Download)
        .collect();
    let mut body = col()
        .w_full()
        .px(12.0)
        .py(12.0)
        .gap(6.0)
        .child(section(&format!("Downloads ({})", downloads.len()), t).pl(4.0));
    if downloads.is_empty() {
        body = body.child(
            text("Nothing downloaded yet. ↓ on a track saves it.")
                .font(type_::LABEL)
                .color(t.text_secondary)
                .pl(4.0),
        );
    }
    for (i, d) in downloads.iter().rev().enumerate() {
        body = body.child(download_row(i, d, t));
    }
    body = body
        .child(el().h(10.0))
        .child(section(&format!("Uploads ({})", s.uploads.len()), t).pl(4.0));
    if s.uploads.is_empty() {
        body = body.child(
            text("No one is downloading from you.")
                .font(type_::LABEL)
                .color(t.text_secondary)
                .pl(4.0),
        );
    }
    for (i, u) in s.uploads.iter().enumerate() {
        let frac = if u.size == 0 {
            0.0
        } else {
            (u.bytes_sent as f64 / u.size as f64) as f32
        };
        let (status, color) = match &u.state {
            UploadState::Queued { place } => (format!("queued #{place}"), t.text_secondary),
            UploadState::InProgress => (
                format!("{:.0}% · {}", frac * 100.0, fmt::speed(u.speed)),
                accent(),
            ),
            UploadState::Completed => ("done".into(), green()),
            UploadState::Cancelled => ("cancelled".into(), t.text_secondary),
            UploadState::Failed(e) => (format!("failed: {e}"), red()),
        };
        body = body.child(
            col()
                .key(format!("up-{i}"))
                .w_full()
                .px(8.0)
                .py(8.0)
                .gap(5.0)
                .radius(7.0)
                .fill(t.card)
                .child(
                    row()
                        .gap(8.0)
                        .center_y()
                        .child(
                            text(goalseek_core::paths::remote_basename(&u.path).to_string())
                                .font(type_::BODY)
                                .color(t.text)
                                .truncate()
                                .grow(1.0)
                                .shrink(),
                        )
                        .child(upload_action(u, t)),
                )
                .child(
                    row()
                        .gap(8.0)
                        .center_y()
                        .child(
                            text(format!("to {}", u.username))
                                .font(type_::LABEL)
                                .color(t.text_secondary)
                                .grow(1.0)
                                .truncate()
                                .shrink(),
                        )
                        .child(text(status).font(tnum(type_::LABEL)).color(color)),
                )
                .child(progress(
                    "bar",
                    frac,
                    W - 40.0,
                    if matches!(u.state, UploadState::Completed) {
                        green()
                    } else {
                        accent()
                    },
                    t,
                )),
        );
    }
    panel("transfers-panel", "Transfers", body, t)
}

fn download_row(i: usize, d: &Transfer, t: &Theme) -> El {
    let frac = d.fraction();
    let (status, color) = match &d.state {
        TState::Queued => ("queued".to_string(), t.text_secondary),
        TState::RemoteQueued(Some(n)) => (format!("queued at peer #{n}"), t.text_secondary),
        TState::RemoteQueued(None) => ("queued at peer".into(), t.text_secondary),
        TState::Active => (
            format!("{:.0}% · {}", frac * 100.0, fmt::speed(d.speed)),
            accent(),
        ),
        TState::Done(_) => ("done".into(), green()),
        TState::Failed(e) => (format!("failed: {e}"), red()),
        TState::Cancelled => ("cancelled".into(), t.text_secondary),
    };
    let id = d.id;
    let action: El = match d.state {
        TState::Queued | TState::RemoteQueued(_) | TState::Active => {
            icon_button("cancel", icons::close(10.0, t.text_secondary), t)
                .size(24.0, 24.0)
                .label("Cancel")
                .on_press(move |s: &mut App| s.cancel(id))
        }
        TState::Failed(_) | TState::Cancelled => {
            icon_button("retry", icons::retry(12.0, t.text), t)
                .size(24.0, 24.0)
                .label("Retry")
                .on_press(move |s: &mut App| s.retry(id))
        }
        TState::Done(_) => el().size(24.0, 24.0),
    };
    let bar_color = match d.state {
        TState::Done(_) => green(),
        TState::Failed(_) => red(),
        _ => accent(),
    };
    col()
        .key(format!("dl-{i}"))
        .w_full()
        .px(8.0)
        .py(8.0)
        .gap(5.0)
        .radius(7.0)
        .fill(t.card)
        .child(
            row()
                .gap(8.0)
                .center_y()
                .child(
                    text(d.file.basename().to_string())
                        .font(type_::BODY)
                        .color(t.text)
                        .truncate()
                        .grow(1.0)
                        .shrink(),
                )
                .child(action),
        )
        .child(
            row()
                .gap(8.0)
                .center_y()
                .child(
                    text(format!("{} · {}", d.file.username, fmt::size(d.total)))
                        .font(type_::LABEL)
                        .color(t.text_secondary)
                        .grow(1.0)
                        .truncate()
                        .shrink(),
                )
                .child(text(status).font(tnum(type_::LABEL)).color(color)),
        )
        .child(progress("bar", frac, W - 40.0, bar_color, t))
}

pub fn settings(s: &App, t: &Theme) -> El {
    let f = &s.settings_form;
    let (account, port) = match &s.conn {
        Conn::Connected {
            username,
            listen_port,
        } => (
            format!("{username} · connected"),
            listen_port.map_or_else(|| "not listening".into(), |p| format!("listening on {p}")),
        ),
        Conn::Reconnecting(m) => (format!("reconnecting ({m})"), String::new()),
        other => (format!("{other:?}").to_lowercase(), String::new()),
    };
    let account = col()
        .gap(6.0)
        .child(section("Account", t))
        .child(
            row()
                .gap(10.0)
                .center_y()
                .child(
                    text(account)
                        .key("set-account")
                        .font(type_::BODY)
                        .color(t.text)
                        .grow(1.0)
                        .truncate()
                        .shrink(),
                )
                .child(button("logout", "Log out", false, t).on_press(|s: &mut App| s.logout(false)))
                .child(button("forget", "Forget account", false, t).on_press(|s: &mut App| s.logout(true))),
        )
        .child(
            text(if s.keyring_available {
                "Your password is kept in the system keyring. “Forget account” removes it."
            } else {
                "No system keyring found: your password is not remembered and you sign in each launch."
            })
            .key("keyring-note")
            .font(type_::LABEL)
            .color(t.text_secondary)
            .wrap(),
        )
        .child(text(port).font(type_::LABEL).color(t.text_secondary))
        .child(
            text(
                s.port_mapping
                    .clone()
                    .unwrap_or_else(|| "Router port mapping: no result yet".into()),
            )
            .key("set-portmap")
            .font(type_::LABEL)
            .color(t.text_secondary)
            .wrap(),
        );

    let mut shares = col().gap(6.0).child(section("Shares", t));
    for (i, dir) in f.shares.iter().enumerate() {
        shares = shares.child(
            row()
                .key(format!("set-share-{i}"))
                .h(28.0)
                .pl(10.0)
                .gap(8.0)
                .center_y()
                .radius(6.0)
                .fill(t.control_track)
                .child(icons::folder(13.0, t.text_secondary))
                .child(
                    text(dir.display().to_string())
                        .font(type_::BODY)
                        .color(t.text)
                        .truncate()
                        .grow(1.0)
                        .shrink(),
                )
                .child(
                    icon_button("remove", icons::close(10.0, t.text_secondary), t)
                        .label("Remove share")
                        .on_press(move |s: &mut App| {
                            if i < s.settings_form.shares.len() {
                                s.settings_form.shares.remove(i);
                            }
                        }),
                ),
        );
    }
    if f.shares.is_empty() {
        shares = shares.child(
            text("Not sharing anything.")
                .font(type_::LABEL)
                .color(t.text_secondary),
        );
    }
    let counts = s.share_counts.map_or_else(
        || "not indexed yet".into(),
        |(d, n)| format!("{d} folders · {n} files shared"),
    );
    shares = shares
        .child(
            row()
                .gap(8.0)
                .child(
                    field_box(
                        "set-share",
                        input(&f.share_input, "Add a folder…", t)
                            .on_edit(|s: &mut App| &mut s.settings_form.share_input)
                            .on_submit(|s: &mut App, _| s.add_settings_share()),
                        t,
                    )
                    .grow(1.0),
                )
                .child(
                    button("set-share-add", "Add", false, t)
                        .on_press(|s: &mut App| s.add_settings_share()),
                ),
        )
        .child(
            row()
                .gap(10.0)
                .center_y()
                .child(
                    text(counts)
                        .key("share-counts")
                        .font(type_::LABEL)
                        .color(t.text_secondary)
                        .grow(1.0),
                )
                .child(button("rescan", "Rescan", false, t).on_press(|s: &mut App| s.rescan())),
        );

    let labeled = |label: &str, field: El| col().gap(6.0).child(section(label, t)).child(field);
    let downloads = labeled(
        "Download folder",
        field_box(
            "set-dl",
            input(&f.download_dir, "~/Downloads/goalseek", t)
                .on_edit(|s: &mut App| &mut s.settings_form.download_dir),
            t,
        ),
    );
    let network = row()
        .gap(12.0)
        .child(
            labeled(
                "Listen port",
                field_box(
                    "set-port",
                    input(&f.listen_port, "2234", t)
                        .on_edit(|s: &mut App| &mut s.settings_form.listen_port),
                    t,
                ),
            )
            .grow(1.0),
        )
        .child(
            labeled(
                "Upload slots",
                field_box(
                    "set-slots",
                    input(&f.upload_slots, "10", t)
                        .on_edit(|s: &mut App| &mut s.settings_form.upload_slots),
                    t,
                ),
            )
            .grow(1.0),
        );

    let mut footer = row()
        .gap(10.0)
        .center_y()
        .child(button("apply", "Save", true, t).on_press(|s: &mut App| s.apply_settings()));
    if let Some((ok, m)) = &f.message {
        footer = footer.child(
            text(m.clone())
                .key("set-message")
                .font(type_::LABEL)
                .color(if *ok { t.text_secondary } else { red() })
                .wrap()
                .grow(1.0)
                .shrink(),
        );
    }
    let body = col().w_full().px(16.0).py(14.0).gap(18.0)
        .child(account)
        .child(shares)
        .child(downloads)
        .child(network)
        .child(text("A port change reconnects. Cache: plays stream into the cache, never into downloads.").font(type_::LABEL).color(t.text_secondary).wrap())
        .child(footer);
    panel("settings-panel", "Settings", body, t)
}

/// × for an upload that can still be stopped.
fn upload_action(u: &goalseek_core::UploadRow, t: &Theme) -> El {
    if !matches!(
        u.state,
        UploadState::Queued { .. } | UploadState::InProgress
    ) {
        return el().size(24.0, 24.0);
    }
    let (user, path) = (u.username.clone(), u.path.clone());
    icon_button("cancel", icons::close(10.0, t.text_secondary), t)
        .size(24.0, 24.0)
        .label("Cancel upload")
        .on_press(move |s: &mut App| s.cancel_upload(&user, &path))
}
