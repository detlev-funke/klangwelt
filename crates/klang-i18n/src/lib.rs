//! Translation of the core's messages.
//!
//! The dictionaries belong to the user interface (`ui/src/i18n/<code>.ts`): at
//! start it loads the chosen language and sends the dictionary to the core with
//! the command `set_language` (same on desktop and in the browser). Keys are –
//! as in the UI – the English texts in the code. Without a dictionary (tests,
//! command line) everything is English.
//!
//! * [`tr!`] – translate a text and fill `{name}` placeholders:
//!   `tr!("Track “{name}” deleted", name = track.name)`.
//! * [`trn!`] – the same with a count: `trn!(n, "one note", "{n} notes")`; the
//!   key is the plural form, `{n}` is always set.
//! * [`tk!`] – marks a text in a table as a key (returns it unchanged); it is
//!   translated later with [`translate`].
//! * [`tid!`] – marks an identifier that is also shown: its value never
//!   changes (it is persisted, compared or sent back) and is translated for
//!   display with [`translate`] – in every dictionary, English included.
//!
//! Format specs like `{:.1}` don't exist in keys: format values beforehand
//! (`bpm = format!("{bpm:.1}")`).
//!
//! Plural forms arrive flat from the UI: `key` is the form "other",
//! `key\u{1f}one` (or `few`, `many`, …) are the others.

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::RwLock;

/// Language of the keys.
pub const SOURCE: &str = "en";

struct Dict {
    lang: String,
    map: HashMap<String, String>,
}

static DICT: RwLock<Option<Dict>> = RwLock::new(None);

/// Set language and dictionary (from the UI).
pub fn set_language(lang: &str, map: HashMap<String, String>) {
    let mut d = DICT.write().unwrap_or_else(|e| e.into_inner());
    *d = Some(Dict { lang: lang.to_string(), map });
}

/// Chosen language (code like "de"); without a choice the source language.
pub fn language() -> String {
    let d = DICT.read().unwrap_or_else(|e| e.into_inner());
    d.as_ref().map(|d| d.lang.clone()).unwrap_or_else(|| SOURCE.to_string())
}

/// Translate a text (without filling placeholders). Unknown texts stay.
pub fn translate(key: &str) -> String {
    let d = DICT.read().unwrap_or_else(|e| e.into_inner());
    d.as_ref().and_then(|d| d.map.get(key)).cloned().unwrap_or_else(|| key.to_string())
}

/// Plural: template for `n` (the key is `other`). Without an entry the forms
/// from the code, with the English rule.
pub fn plural_template(n: f64, one: &str, other: &str) -> String {
    let fallback = || if n == 1.0 { one } else { other }.to_string();
    let d = DICT.read().unwrap_or_else(|e| e.into_inner());
    let Some(d) = d.as_ref() else { return fallback() };
    let cat = category(&d.lang, n);
    d.map.get(&format!("{other}\u{1f}{cat}")).or_else(|| d.map.get(other)).cloned().unwrap_or_else(fallback)
}

/// Plural category like `Intl.PluralRules` (for whole numbers; otherwise "other").
/// A new language with other plural rules than English needs a branch here.
pub fn category(lang: &str, n: f64) -> &'static str {
    if n.fract() != 0.0 || !n.is_finite() {
        return "other";
    }
    let i = n.abs() as u64;
    match lang {
        "fr" => {
            if i <= 1 {
                "one"
            } else if i != 0 && i % 1_000_000 == 0 {
                "many"
            } else {
                "other"
            }
        }
        "pl" => {
            if i == 1 {
                "one"
            } else if (2..=4).contains(&(i % 10)) && !(12..=14).contains(&(i % 100)) {
                "few"
            } else {
                "many"
            }
        }
        _ => {
            if i == 1 {
                "one"
            } else {
                "other"
            }
        }
    }
}

/// A decimal number for a message in the chosen language: "0.5" in English,
/// "0,5" in the other languages Klangwelt speaks.
pub fn decimal(x: f64) -> String {
    let s = format!("{x}");
    if language() == "en" {
        s
    } else {
        s.replace('.', ",")
    }
}

/// Fill `{name}` placeholders; unknown ones stay as they are.
pub fn fill(text: &str, vars: &[(&str, &dyn Display)]) -> String {
    if vars.is_empty() || !text.contains('{') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + 16);
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('}') {
            Some(end) => {
                let name = &after[..end];
                match vars.iter().find(|(k, _)| *k == name) {
                    Some((_, v)) => out.push_str(&v.to_string()),
                    None => {
                        out.push('{');
                        out.push_str(name);
                        out.push('}');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Translate a text and fill placeholders: `tr!("Bar {n}", n = bar)`.
#[macro_export]
macro_rules! tr {
    ($key:literal) => {
        $crate::translate($key)
    };
    ($key:literal, $($name:ident = $val:expr),+ $(,)?) => {
        $crate::fill(&$crate::translate($key), &[$((stringify!($name), &$val as &dyn ::std::fmt::Display)),+])
    };
}

/// Plural: `trn!(n, "one note", "{n} notes")` (more placeholders as with [`tr!`]).
#[macro_export]
macro_rules! trn {
    ($n:expr, $one:literal, $other:literal $(, $name:ident = $val:expr)* $(,)?) => {{
        let n = $n;
        $crate::fill(
            &$crate::plural_template(n as f64, $one, $other),
            &[("n", &n as &dyn ::std::fmt::Display) $(, (stringify!($name), &$val as &dyn ::std::fmt::Display))*],
        )
    }};
}

/// Marks a text as a key (for the key list); returns it unchanged.
#[macro_export]
macro_rules! tk {
    ($key:literal) => {
        $key
    };
}

/// Marks an identifier that is also shown to the user (a preset name, a facet
/// value in the score index …): its value must not change (it is persisted,
/// compared or sent back), so it stays as it is – German for historical
/// reasons – and every dictionary, English included, translates it for
/// display with [`translate`]. Returns it unchanged.
#[macro_export]
macro_rules! tid {
    ($id:literal) => {
        $id
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    // One test, because the dictionary is global (tests would run in parallel otherwise).
    #[test]
    fn translates_fills_and_counts() {
        assert_eq!(tr!("Track {name}", name = "Bass"), "Track Bass");
        assert_eq!(trn!(1, "one note", "{n} notes"), "one note");
        assert_eq!(trn!(3, "one note", "{n} notes"), "3 notes");
        assert_eq!(fill("{a} and {b}", &[("a", &1)]), "1 and {b}");
        assert_eq!(fill("open {a", &[("a", &1)]), "open {a");
        assert_eq!(tk!("Major"), "Major");
        assert_eq!(tid!("Glocke"), "Glocke");

        let mut map = HashMap::new();
        map.insert("Track {name}".to_string(), "Spur {name}".to_string());
        map.insert("{n} notes".to_string(), "{n} Noten".to_string());
        map.insert("{n} notes\u{1f}one".to_string(), "eine Note".to_string());
        set_language("de", map.clone());
        assert_eq!(language(), "de");
        assert_eq!(tr!("Track {name}", name = "Bass"), "Spur Bass");
        assert_eq!(tr!("Unknown"), "Unknown");
        assert_eq!(trn!(1, "one note", "{n} notes"), "eine Note");
        assert_eq!(trn!(2, "one note", "{n} notes"), "2 Noten");

        map.insert("{n} notes\u{1f}few".to_string(), "{n} nuty".to_string());
        map.insert("{n} notes\u{1f}many".to_string(), "{n} nut".to_string());
        set_language("pl", map);
        assert_eq!(trn!(3, "one note", "{n} notes"), "3 nuty");
        assert_eq!(trn!(5, "one note", "{n} notes"), "5 nut");
        assert_eq!(trn!(22, "one note", "{n} notes"), "22 nuty");
        assert_eq!(trn!(12, "one note", "{n} notes"), "12 nut");

        // English with only identifiers in its dictionary: texts as in the code
        let mut ids = HashMap::new();
        ids.insert("Glocke".to_string(), "Bell".to_string());
        set_language("en", ids);
        assert_eq!(translate(tid!("Glocke")), "Bell");
        assert_eq!(trn!(1, "one note", "{n} notes"), "one note");
        assert_eq!(trn!(4, "one note", "{n} notes"), "4 notes");
        assert_eq!(category("fr", 0.0), "one");
        assert_eq!(category("en", 0.0), "other");
        assert_eq!(category("en", 1.5), "other");
    }
}
