//! User settings from `~/.config/noida/settings.json`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// "dark" or "light".
    pub theme: String,
    pub tab_width: usize,
    pub word_wrap: bool,
    pub auto_save: bool,
    pub auto_close_brackets: bool,
    pub sticky_scroll: bool,
    pub breadcrumbs: bool,
    /// Tell agents which file/lines you are looking at when you send a prompt.
    pub share_editor_context: bool,
    /// Extra Alt shortcuts, e.g. {"alt+y": "command_palette"}. Action ids are
    /// listed by "Help: Keyboard Shortcuts".
    pub keys: HashMap<String, String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "dark".into(),
            tab_width: 4,
            word_wrap: false,
            auto_save: false,
            auto_close_brackets: true,
            sticky_scroll: true,
            breadcrumbs: true,
            share_editor_context: true,
            keys: HashMap::new(),
        }
    }
}

static TAB_WIDTH: AtomicUsize = AtomicUsize::new(4);
static AUTO_CLOSE: AtomicBool = AtomicBool::new(true);

pub fn tab_width() -> usize {
    TAB_WIDTH.load(Ordering::Relaxed).clamp(1, 16)
}

pub fn auto_close() -> bool {
    AUTO_CLOSE.load(Ordering::Relaxed)
}

/// The user's home directory. Windows spells it `USERPROFILE`.
pub fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from)
}

pub fn path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join(".config")))?;
    Some(base.join("noida/settings.json"))
}

/// Load settings (defaults when missing) and apply the global ones.
pub fn load() -> Result<Settings, String> {
    let s = match path().and_then(|p| std::fs::read_to_string(p).ok()) {
        Some(text) => serde_json::from_str(&text).map_err(|e| format!("settings.json: {e}"))?,
        None => Settings::default(),
    };
    apply(&s);
    Ok(s)
}

pub fn apply(s: &Settings) {
    TAB_WIDTH.store(s.tab_width, Ordering::Relaxed);
    AUTO_CLOSE.store(s.auto_close_brackets, Ordering::Relaxed);
    crate::theme::set_light(s.theme.eq_ignore_ascii_case("light"));
}

/// Write the defaults so the user has something to edit; returns the path.
pub fn ensure_file() -> Option<PathBuf> {
    let p = path()?;
    if !p.exists() {
        std::fs::create_dir_all(p.parent()?).ok()?;
        std::fs::write(&p, serde_json::to_string_pretty(&Settings::default()).ok()?).ok()?;
    }
    Some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_settings_fill_defaults() {
        let s: Settings = serde_json::from_str(r#"{"theme": "light", "keys": {"alt+y": "quick_open"}}"#).unwrap();
        assert_eq!(s.theme, "light");
        assert_eq!(s.tab_width, 4);
        assert!(s.auto_close_brackets);
        assert_eq!(s.keys["alt+y"], "quick_open");
    }
}
