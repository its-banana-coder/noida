//! Key spec strings for custom keybindings ("alt+y", "ctrl+shift+p", "f5")
//! and normalisation of key events across legacy and kitty keyboard encodings.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::actions::Action;

static ENHANCED: AtomicBool = AtomicBool::new(false);

/// Whether the kitty keyboard protocol was enabled, so Ctrl+Shift combos can arrive.
pub fn enhanced() -> bool {
    ENHANCED.load(Ordering::Relaxed)
}

pub fn set_enhanced(on: bool) {
    ENHANCED.store(on, Ordering::Relaxed)
}

/// Make shifted letters look the same whatever the terminal sends: legacy
/// terminals report Alt+Shift+s as `Char('S')`+ALT(+SHIFT), the kitty protocol as
/// `Char('s')`+ALT+SHIFT, or `Char('S')`+ALT with alternate keys. All become
/// `Char('S')` with SHIFT set.
pub fn normalize(mut key: KeyEvent) -> KeyEvent {
    if let KeyCode::Char(c) = key.code {
        if c.is_uppercase() {
            key.modifiers |= KeyModifiers::SHIFT;
        } else if c.is_lowercase() && key.modifiers.contains(KeyModifiers::SHIFT) {
            let mut up = c.to_uppercase();
            if let (Some(u), None) = (up.next(), up.next()) {
                key.code = KeyCode::Char(u);
            }
        }
    }
    key
}

/// Canonical spec of a key event: lowercase, modifiers in ctrl, alt, shift order.
pub fn spec(key: KeyEvent) -> Option<String> {
    let key = normalize(key);
    let m = key.modifiers;
    if m.intersects(KeyModifiers::SUPER | KeyModifiers::HYPER | KeyModifiers::META) {
        return None;
    }
    let mut shift = m.contains(KeyModifiers::SHIFT);
    let name = match key.code {
        KeyCode::Char(' ') => "space".to_string(),
        KeyCode::Char('+') => "plus".to_string(),
        KeyCode::Char(c) => c.to_lowercase().collect(),
        KeyCode::F(n) => format!("f{n}"),
        KeyCode::BackTab => {
            shift = true;
            "tab".into()
        }
        code => named(code)?.into(),
    };
    Some(join(m.contains(KeyModifiers::CONTROL), m.contains(KeyModifiers::ALT), shift, &name))
}

/// Canonicalise a user-written spec such as "Alt+S" or "ctrl+shift+P".
pub fn parse(s: &str) -> Option<String> {
    let s = s.trim().to_string();
    // A literal '+' key: "alt++" or "+".
    let (mods, key) = match s.strip_suffix("++") {
        Some(rest) => (rest, "+"),
        None if s == "+" => ("", "+"),
        None => s.rsplit_once('+').unwrap_or(("", &s)),
    };
    let (mut ctrl, mut alt, mut shift) = (false, false, false);
    for m in mods.split('+').filter(|m| !m.is_empty()) {
        match m.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => ctrl = true,
            "alt" | "meta" | "option" | "opt" => alt = true,
            "shift" => shift = true,
            _ => return None,
        }
    }
    let mut chars = key.chars();
    let name = match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_uppercase() => {
            shift = true;
            c.to_lowercase().collect()
        }
        (Some('+'), None) => "plus".into(),
        (Some(' '), None) => "space".into(),
        (Some(c), None) => c.to_string(),
        _ => {
            let k = key.to_ascii_lowercase();
            let k = match k.as_str() {
                "escape" => "esc".to_string(),
                "return" => "enter".into(),
                "del" => "delete".into(),
                "ins" => "insert".into(),
                "pgup" => "pageup".into(),
                "pgdn" | "pgdown" => "pagedown".into(),
                _ => k,
            };
            let valid = ["space", "plus", "tab", "backtab"].contains(&k.as_str())
                || NAMED.iter().any(|(_, n)| *n == k)
                || k.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()).is_some_and(|n| (1..=24).contains(&n));
            if !valid {
                return None;
            }
            if k == "backtab" {
                shift = true;
                "tab".into()
            } else {
                k
            }
        }
    };
    Some(join(ctrl, alt, shift, &name))
}

/// Title-cased spec for hints: "ctrl+shift+p" → "Ctrl+Shift+P".
pub fn display(spec: &str) -> String {
    let title = |p: &str| {
        let mut c = p.chars();
        c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
    };
    spec.split('+').map(title).collect::<Vec<String>>().join("+")
}

const NAMED: &[(KeyCode, &str)] = &[
    (KeyCode::Enter, "enter"),
    (KeyCode::Esc, "esc"),
    (KeyCode::Tab, "tab"),
    (KeyCode::Backspace, "backspace"),
    (KeyCode::Delete, "delete"),
    (KeyCode::Insert, "insert"),
    (KeyCode::Home, "home"),
    (KeyCode::End, "end"),
    (KeyCode::PageUp, "pageup"),
    (KeyCode::PageDown, "pagedown"),
    (KeyCode::Up, "up"),
    (KeyCode::Down, "down"),
    (KeyCode::Left, "left"),
    (KeyCode::Right, "right"),
];

fn named(code: KeyCode) -> Option<&'static str> {
    NAMED.iter().find(|(c, _)| *c == code).map(|(_, n)| *n)
}

fn join(ctrl: bool, alt: bool, shift: bool, name: &str) -> String {
    let mut s = String::new();
    for (on, m) in [(ctrl, "ctrl+"), (alt, "alt+"), (shift, "shift+")] {
        if on {
            s.push_str(m);
        }
    }
    s + name
}

/// What a key is bound to in settings: an action, or `None` to disable the built-in shortcut.
pub type Keymap = HashMap<String, Option<Action>>;

/// Build the keymap from `settings.keys`, returning descriptions of bad entries.
pub fn keymap(keys: &HashMap<String, String>) -> (Keymap, Vec<String>) {
    let mut map = Keymap::new();
    let mut bad = Vec::new();
    let mut entries: Vec<_> = keys.iter().collect();
    entries.sort();
    for (k, id) in entries {
        let Some(spec) = parse(k) else {
            bad.push(format!("bad key \"{k}\""));
            continue;
        };
        let id = id.trim();
        if id.is_empty() || id.eq_ignore_ascii_case("none") {
            map.insert(spec, None);
        } else if let Some(a) = Action::from_id(id) {
            map.insert(spec, Some(a));
        } else {
            bad.push(format!("unknown action \"{id}\" for {k}"));
        }
    }
    (map, bad)
}

/// Built-in Ctrl+Shift shortcuts. Only reachable with the kitty keyboard
/// protocol; legacy terminals send Ctrl+Shift+P as Ctrl+P.
pub fn ctrl_shift_action(key: KeyEvent) -> Option<Action> {
    let key = normalize(key);
    if !key.modifiers.contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT) || key.modifiers.contains(KeyModifiers::ALT) {
        return None;
    }
    Some(match key.code {
        KeyCode::Char('P') => Action::CommandPalette,
        KeyCode::Char('F') => Action::SearchWorkspace,
        KeyCode::Char('H') => Action::ReplaceWorkspace,
        KeyCode::Char('O') => Action::GoToSymbol,
        KeyCode::Char('T') => Action::ReopenClosedFile,
        KeyCode::Char('E') => Action::FocusTree,
        _ => return None,
    })
}

/// Default Ctrl+Shift hint for the palette, shown only when the protocol is active.
pub fn ctrl_shift_hint(action: Action) -> Option<&'static str> {
    Some(match action {
        Action::CommandPalette => "Ctrl+Shift+P",
        Action::SearchWorkspace => "Ctrl+Shift+F",
        Action::ReplaceWorkspace => "Ctrl+Shift+H",
        Action::GoToSymbol => "Ctrl+Shift+O",
        Action::ReopenClosedFile => "Ctrl+Shift+T",
        Action::FocusTree => "Ctrl+Shift+E",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, m)
    }

    #[test]
    fn canonical_specs() {
        let (a, c, s) = (KeyModifiers::ALT, KeyModifiers::CONTROL, KeyModifiers::SHIFT);
        assert_eq!(spec(ev(KeyCode::Char('y'), a)).unwrap(), "alt+y");
        // Legacy, kitty and kitty-with-alternate-keys encodings of Alt+Shift+s.
        assert_eq!(spec(ev(KeyCode::Char('S'), a)).unwrap(), "alt+shift+s");
        assert_eq!(spec(ev(KeyCode::Char('S'), a | s)).unwrap(), "alt+shift+s");
        assert_eq!(spec(ev(KeyCode::Char('s'), a | s)).unwrap(), "alt+shift+s");
        assert_eq!(spec(ev(KeyCode::Char('k'), c | a)).unwrap(), "ctrl+alt+k");
        assert_eq!(spec(ev(KeyCode::Char('p'), c | s)).unwrap(), "ctrl+shift+p");
        assert_eq!(spec(ev(KeyCode::F(5), KeyModifiers::NONE)).unwrap(), "f5");
        assert_eq!(spec(ev(KeyCode::BackTab, s)).unwrap(), "shift+tab");
        assert_eq!(spec(ev(KeyCode::Char('/'), a)).unwrap(), "alt+/");
        assert_eq!(spec(ev(KeyCode::Char(' '), c)).unwrap(), "ctrl+space");

        assert_eq!(parse("alt+y").unwrap(), "alt+y");
        assert_eq!(parse("Alt+S").unwrap(), "alt+shift+s");
        assert_eq!(parse("shift+alt+s").unwrap(), "alt+shift+s");
        assert_eq!(parse("Ctrl+Shift+P").unwrap(), "ctrl+shift+p");
        assert_eq!(parse("alt+ctrl+k").unwrap(), "ctrl+alt+k");
        assert_eq!(parse("F5").unwrap(), "f5");
        assert_eq!(parse("ctrl+PgDn").unwrap(), "ctrl+pagedown");
        assert_eq!(parse("alt++").unwrap(), "alt+plus");
        assert!(parse("hyper+x").is_none());
        assert!(parse("alt+nope").is_none());
        assert!(parse("").is_none());
        for key in [ev(KeyCode::Char('S'), a), ev(KeyCode::Char('p'), c | s), ev(KeyCode::PageUp, c), ev(KeyCode::Char('+'), a)] {
            assert_eq!(parse(&spec(key).unwrap()), spec(key));
        }
        assert_eq!(display("ctrl+shift+p"), "Ctrl+Shift+P");
    }

    #[test]
    fn keymap_reports_bad_entries() {
        let keys: HashMap<String, String> =
            [("alt+y", "quick_open"), ("Alt+N", "none"), ("alt+x", ""), ("alt+nope", "quit"), ("alt+u", "bogus")].map(|(k, v)| (k.to_string(), v.to_string())).into();
        let (map, bad) = keymap(&keys);
        assert_eq!(map["alt+y"], Some(Action::QuickOpen));
        assert_eq!(map["alt+shift+n"], None);
        assert_eq!(map["alt+x"], None);
        assert_eq!(bad.len(), 2, "{bad:?}");
    }

    #[test]
    fn ctrl_shift_defaults() {
        let cs = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        assert_eq!(ctrl_shift_action(ev(KeyCode::Char('P'), cs)), Some(Action::CommandPalette));
        assert_eq!(ctrl_shift_action(ev(KeyCode::Char('p'), cs)), Some(Action::CommandPalette));
        // Kitty with alternate keys drops SHIFT and reports the shifted letter.
        assert_eq!(ctrl_shift_action(ev(KeyCode::Char('P'), KeyModifiers::CONTROL)), Some(Action::CommandPalette));
        // Legacy terminals send Ctrl+Shift+P as plain Ctrl+P.
        assert_eq!(ctrl_shift_action(ev(KeyCode::Char('p'), KeyModifiers::CONTROL)), None);
    }
}
