use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const DEFAULT_WALLPAPER: &str = "/usr/share/backgrounds/nimbus/default.png";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Path to the wallpaper image.
    pub wallpaper: String,
    /// Prefer dark styling for GTK apps.
    pub dark: bool,
    /// Desktop file IDs pinned to the panel. `a.desktop|b.desktop` means
    /// "the first of these that is installed".
    pub pinned: Vec<String>,
    /// strftime-style format for the panel clock.
    pub clock_format: String,
    /// Command used to lock the screen.
    pub lock_command: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            wallpaper: DEFAULT_WALLPAPER.to_string(),
            dark: true,
            pinned: vec![
                "org.codeberg.dnkl.foot.desktop|foot.desktop".into(),
                "thunar.desktop".into(),
                "firefox.desktop".into(),
            ],
            clock_format: "%a %d %b   %H:%M".into(),
            lock_command: "swaylock -f -c 16161e".into(),
        }
    }
}

pub fn path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("nimbus").join("shell.toml")
}

impl Config {
    pub fn load() -> Self {
        std::fs::read_to_string(path())
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let p = path();
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(p, text)
    }
}

/// Tell GTK / libadwaita apps which colour scheme to use.
pub fn apply_color_scheme(dark: bool) {
    let scheme = if dark { "prefer-dark" } else { "default" };
    let theme = if dark { "Adwaita-dark" } else { "Adwaita" };
    crate::sys::run_quiet("gsettings", &["set", "org.gnome.desktop.interface", "color-scheme", scheme]);
    crate::sys::run_quiet("gsettings", &["set", "org.gnome.desktop.interface", "gtk-theme", theme]);
}
