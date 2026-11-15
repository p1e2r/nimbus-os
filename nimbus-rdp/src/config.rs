//! `~/.config/nimbus/rdp.toml`, written by Nimbus Settings.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Start the server when the session starts.
    pub enabled: bool,
    /// TCP port to listen on.
    pub port: u16,
    /// Login name RDP clients must use.
    pub username: String,
    /// Password RDP clients must use. Stored readable only by the user.
    pub password: String,
    /// Only show the screen; ignore keyboard and mouse from the client.
    pub view_only: bool,
    /// Frames per second to capture while a client is connected.
    pub fps: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: false,
            port: 3389,
            username: std::env::var("USER").unwrap_or_else(|_| "nimbus".into()),
            password: String::new(),
            view_only: false,
            fps: 20,
        }
    }
}

fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("nimbus")
}

pub fn data_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("nimbus/rdp")
}

impl Config {
    pub fn path() -> PathBuf {
        config_dir().join("rdp.toml")
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }
}
