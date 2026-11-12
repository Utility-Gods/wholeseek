//! First launch: Soulseek account, share folders, Connect.

use rune::tok::{TextStyle, Theme, type_};
use rune::{El, col, el, row, text};

use super::{button, field_box, icon_button, icons, input, red, section};
use crate::app::App;

pub fn view(s: &App, t: &Theme) -> El {
    let ob = &s.onboarding;
    let user = field_box(
        "ob-user",
        input(&ob.username, "Username", t)
            .on_edit(|s: &mut App| &mut s.onboarding.username)
            .on_submit(|s: &mut App, _| s.connect()),
        t,
    );
    let pass = field_box(
        "ob-pass",
        input(&ob.password, "Password", t)
            .on_edit(|s: &mut App| &mut s.onboarding.password)
            .on_submit(|s: &mut App, _| s.connect()),
        t,
    );
    let mut shares = col().key("ob-shares").gap(4.0);
    for (i, dir) in ob.shares.iter().enumerate() {
        shares = shares.child(
            row()
                .key(format!("ob-share-{i}"))
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
                            if i < s.onboarding.shares.len() {
                                s.onboarding.shares.remove(i);
                            }
                        }),
                ),
        );
    }
    let add = row()
        .gap(8.0)
        .child(
            field_box(
                "ob-share",
                input(&ob.share_input, "/home/you/Music", t)
                    .on_edit(|s: &mut App| &mut s.onboarding.share_input)
                    .on_submit(|s: &mut App, _| s.add_onboarding_share()),
                t,
            )
            .grow(1.0),
        )
        .child(button("ob-add", "Add", false, t).on_press(|s: &mut App| s.add_onboarding_share()));
    let connect_label = if ob.connecting {
        "Connecting…"
    } else {
        "Connect"
    };
    let connect = button("connect", connect_label, true, t)
        .w_full()
        .h(34.0)
        .on_press(|s: &mut App| s.connect());

    let mut card = col().key("onboarding").w(420.0).padding(28.0).gap(10.0).radius(12.0).fill(t.card).border(1.0, t.card_border)
        .child(text("wholeseek").font(TextStyle::new(20.0, 600.0)).color(t.text))
        .child(text("Sign in with your Soulseek account. A new username is registered on first login.").font(type_::BODY).color(t.text_secondary).wrap())
        .child(el().h(6.0))
        .child(section("Username", t))
        .child(user)
        .child(section("Password", t))
        .child(pass)
        .child(el().h(6.0))
        .child(section("Share folders", t))
        .child(shares)
        .child(add)
        .child(text("Peers can browse and download what you share. You can change this later in Settings.").font(type_::LABEL).color(t.text_secondary).wrap());
    if let Some(e) = &ob.error {
        card = card.child(
            text(e.clone())
                .key("ob-error")
                .font(type_::LABEL)
                .color(red())
                .wrap(),
        );
    }
    card = card.child(el().h(4.0)).child(connect);
    if !s.keyring_available {
        card = card.child(
            text("No system keyring found — you'll sign in each time wholeseek starts.")
                .key("ob-keyring")
                .font(type_::LABEL)
                .color(t.text_secondary)
                .wrap(),
        );
    }
    if s.demo {
        card = card.child(
            text("Demo mode — no network. Any password but “wrong” signs in.")
                .font(type_::LABEL)
                .color(t.text_secondary.alpha(0.7)),
        );
    }
    col().w_full().h_full().center().fill(t.bg).child(card)
}
