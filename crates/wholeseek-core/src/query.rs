//! Outgoing search-query normalization.
//!
//! Peers match a query word by word against their own index of path words.
//! Most clients (Nicotine+, see `pynicotine/search.py`) split file names on
//! punctuation, so `Wasn't Me.flac` is indexed as `wasn`, `t`, `me`, and a
//! query word `wasn't` or `ac/dc` matches nothing on them. This module turns a
//! typed query into the form peers can match, keeping Soulseek's operators.

/// Characters replaced by a space in ordinary query words. ASCII punctuation
/// plus the typographic quotes, dashes and ellipsis people paste from titles.
const PUNCT: &[char] = &[
    '!', '"', '#', '$', '%', '&', '\'', '(', ')', '*', '+', ',', '-', '.', '/', ':', ';', '<', '=',
    '>', '?', '@', '[', '\\', ']', '^', '_', '`', '{', '|', '}', '~', '‘', '’', '‚', '‛', '“', '”',
    '„', '‐', '‑', '‒', '–', '—', '―', '…', '¡', '¿', '«', '»', '·',
];

fn is_punct(c: char) -> bool {
    PUNCT.contains(&c)
}

/// The query actually sent to the network for what the user typed.
///
/// * Ordinary words: punctuation becomes a space (`wasn't` → `wasn t`,
///   `ac/dc` → `ac dc`, `p!nk` → `p nk`), case and accents are kept.
/// * `-word` (exclusion) and `*part` (partial-word wildcard) are sent
///   unchanged, as Nicotine+ does. A bare `-` or `*` is dropped.
/// * `"a phrase"` quotes are removed and the words cleaned like any other.
/// * Whitespace is collapsed. If nothing searchable is left (e.g. `?!`), the
///   trimmed original is returned so a search is never empty.
#[must_use]
pub fn normalize_query(typed: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for word in typed.split_whitespace() {
        let mut chars = word.chars();
        if matches!(chars.next(), Some('-' | '*')) && chars.next().is_some() {
            out.push(word.to_string());
            continue;
        }
        out.extend(
            word.split(is_punct)
                .filter(|p| !p.is_empty())
                .map(str::to_string),
        );
    }
    let joined = out.join(" ");
    if joined.is_empty() {
        typed.trim().to_string()
    } else {
        joined
    }
}

/// True when `typed` uses Soulseek search syntax (`-exclude`, `*partial`,
/// `"phrase"`); such queries are never broadened.
#[must_use]
pub fn has_operators(typed: &str) -> bool {
    typed.contains('"')
        || typed.split_whitespace().any(|w| {
            let mut c = w.chars();
            matches!(c.next(), Some('-' | '*')) && c.next().is_some()
        })
}

/// The one broader query tried when `sent` (a [`normalize_query`] result)
/// found no audio: accents folded and one-character words dropped
/// (`wasn t me` → `wasn me`, `sigur rós` → `sigur ros`). `None` when that
/// changes nothing or leaves nothing.
#[must_use]
pub fn broaden_query(sent: &str) -> Option<String> {
    let folded = fold_accents(sent);
    let words: Vec<&str> = folded
        .split_whitespace()
        .filter(|w| w.chars().count() > 1)
        .collect();
    let b = words.join(" ");
    (!b.is_empty() && b != sent).then_some(b)
}

/// Fold Latin accents to ASCII letters (`é` → `e`, `ø` → `o`, `ß` → `ss`).
/// Characters outside the table are kept. Not used by [`normalize_query`]:
/// see `docs/core-api.md` §3.10 for the measurements behind that.
#[must_use]
pub fn fold_accents(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match fold_char(c) {
            Some(r) => out.push_str(r),
            None => out.push(c),
        }
    }
    out
}

fn fold_char(c: char) -> Option<&'static str> {
    Some(match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
        'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' | 'Ā' | 'Ă' | 'Ą' => "A",
        'æ' => "ae",
        'Æ' => "AE",
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => "c",
        'Ç' | 'Ć' | 'Ĉ' | 'Ċ' | 'Č' => "C",
        'ď' | 'đ' | 'ð' => "d",
        'Ď' | 'Đ' | 'Ð' => "D",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
        'È' | 'É' | 'Ê' | 'Ë' | 'Ē' | 'Ĕ' | 'Ė' | 'Ę' | 'Ě' => "E",
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => "g",
        'Ĝ' | 'Ğ' | 'Ġ' | 'Ģ' => "G",
        'ĥ' | 'ħ' => "h",
        'Ĥ' | 'Ħ' => "H",
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => "i",
        'Ì' | 'Í' | 'Î' | 'Ï' | 'Ĩ' | 'Ī' | 'Ĭ' | 'Į' | 'İ' => "I",
        'ĵ' => "j",
        'Ĵ' => "J",
        'ķ' => "k",
        'Ķ' => "K",
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => "l",
        'Ĺ' | 'Ļ' | 'Ľ' | 'Ŀ' | 'Ł' => "L",
        'ñ' | 'ń' | 'ņ' | 'ň' => "n",
        'Ñ' | 'Ń' | 'Ņ' | 'Ň' => "N",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => "o",
        'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' | 'Ø' | 'Ō' | 'Ŏ' | 'Ő' => "O",
        'œ' => "oe",
        'Œ' => "OE",
        'ŕ' | 'ŗ' | 'ř' => "r",
        'Ŕ' | 'Ŗ' | 'Ř' => "R",
        'ś' | 'ŝ' | 'ş' | 'š' => "s",
        'Ś' | 'Ŝ' | 'Ş' | 'Š' => "S",
        'ß' => "ss",
        'ţ' | 'ť' | 'ŧ' => "t",
        'Ţ' | 'Ť' | 'Ŧ' => "T",
        'þ' => "th",
        'Þ' => "TH",
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => "u",
        'Ù' | 'Ú' | 'Û' | 'Ü' | 'Ũ' | 'Ū' | 'Ŭ' | 'Ů' | 'Ű' | 'Ų' => "U",
        'ŵ' => "w",
        'Ŵ' => "W",
        'ý' | 'ÿ' | 'ŷ' => "y",
        'Ý' | 'Ÿ' | 'Ŷ' => "Y",
        'ź' | 'ż' | 'ž' => "z",
        'Ź' | 'Ż' | 'Ž' => "Z",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn punctuation_inside_words_becomes_a_space() {
        assert_eq!(normalize_query("wasn't me shaggy"), "wasn t me shaggy");
        assert_eq!(normalize_query("Wazn’t Me"), "Wazn t Me");
        assert_eq!(normalize_query("don't stop me now"), "don t stop me now");
        assert_eq!(normalize_query("guns n' roses"), "guns n roses");
        assert_eq!(
            normalize_query("ac/dc back in black"),
            "ac dc back in black"
        );
        assert_eq!(normalize_query("p!nk"), "p nk");
        assert_eq!(normalize_query("  sigur   rós  "), "sigur rós");
        assert_eq!(normalize_query("beyoncé halo"), "beyoncé halo");
        assert_eq!(
            normalize_query("artist – title (live)"),
            "artist title live"
        );
        assert_eq!(normalize_query("a.b.c"), "a b c");
    }

    #[test]
    fn operators_are_kept() {
        assert_eq!(normalize_query("shaggy -remix"), "shaggy -remix");
        assert_eq!(normalize_query("*aggy wasn't"), "*aggy wasn t");
        assert_eq!(normalize_query("x -don't"), "x -don't");
        // Bare operators and a dash between words are not operators.
        assert_eq!(normalize_query("artist - title"), "artist title");
        assert_eq!(normalize_query("a * b"), "a b");
        assert_eq!(normalize_query("--"), "--");
        assert_eq!(normalize_query("- *"), "- *");
    }

    #[test]
    fn phrase_quotes_are_dropped() {
        assert_eq!(normalize_query("\"wasn't me\" shaggy"), "wasn t me shaggy");
    }

    #[test]
    fn never_empty() {
        assert_eq!(normalize_query("  ?!  "), "?!");
        assert_eq!(normalize_query("   "), "");
        assert_eq!(normalize_query("!!! (band)"), "band");
    }

    #[test]
    fn operators_are_detected() {
        assert!(has_operators("shaggy -remix"));
        assert!(has_operators("*aggy"));
        assert!(has_operators("\"wasn't me\""));
        assert!(!has_operators("artist - title"));
        assert!(!has_operators("wasn't me"));
    }

    #[test]
    fn broaden_folds_accents_and_drops_single_letters() {
        assert_eq!(
            broaden_query("wasn t me shaggy").as_deref(),
            Some("wasn me shaggy")
        );
        assert_eq!(broaden_query("sigur rós").as_deref(), Some("sigur ros"));
        assert_eq!(broaden_query("p nk").as_deref(), Some("nk"));
        assert_eq!(broaden_query("aphex twin"), None);
        assert_eq!(broaden_query("a b"), None);
    }

    #[test]
    fn fold_accents_maps_latin_letters() {
        assert_eq!(fold_accents("beyoncé sigur rós"), "beyonce sigur ros");
        assert_eq!(
            fold_accents("Mötley Crüe, Ørkenstraße"),
            "Motley Crue, Orkenstrasse"
        );
        assert_eq!(fold_accents("日本"), "日本");
    }
}
