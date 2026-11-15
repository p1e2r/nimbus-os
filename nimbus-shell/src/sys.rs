//! Small helpers that talk to the rest of the system.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

/// Run a shell command in the background without blocking the UI.
pub fn spawn(cmd: &str) {
    match Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .stdin(Stdio::null())
        .spawn()
    {
        Ok(mut child) => {
            // Reap it so we don't leave zombies around.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => eprintln!("nimbus-shell: failed to run `{cmd}`: {e}"),
    }
}

/// Start another role of this binary, e.g. `spawn_self("launcher")`.
pub fn spawn_self(role: &str) {
    let exe = std::env::current_exe().unwrap_or_else(|_| "nimbus-shell".into());
    match Command::new(exe).arg(role).stdin(Stdio::null()).spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => eprintln!("nimbus-shell: failed to start {role}: {e}"),
    }
}

/// Run a program and wait for it, ignoring output.
pub fn run_quiet(prog: &str, args: &[&str]) {
    let _ = Command::new(prog)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn output(prog: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(prog).args(args).stderr(Stdio::null()).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Replace this process with swaybg showing the configured wallpaper.
pub fn exec_wallpaper(cfg: &crate::config::Config) {
    use std::os::unix::process::CommandExt;
    let path = if Path::new(&cfg.wallpaper).exists() {
        cfg.wallpaper.clone()
    } else {
        crate::config::DEFAULT_WALLPAPER.to_string()
    };
    let err = Command::new("swaybg").args(["-m", "fill", "-i", &path]).exec();
    eprintln!("nimbus-shell: could not start swaybg: {err}");
}

/// Restart the wallpaper process after the setting changed.
pub fn restart_wallpaper() {
    run_quiet("pkill", &["-x", "swaybg"]);
    spawn_self("wallpaper");
}

// ---------------------------------------------------------------- audio

/// Volume from 0.0 to 1.5 and whether the default sink is muted.
pub fn volume() -> Option<(f64, bool)> {
    let out = output("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"])?;
    parse_wpctl(&out)
}

fn parse_wpctl(out: &str) -> Option<(f64, bool)> {
    // "Volume: 0.40" or "Volume: 0.40 [MUTED]"
    let rest = out.trim().strip_prefix("Volume:")?.trim();
    let num = rest.split_whitespace().next()?.parse::<f64>().ok()?;
    Some((num, rest.contains("[MUTED]")))
}

pub fn set_volume(v: f64) {
    let v = format!("{:.2}", v.clamp(0.0, 1.0));
    run_quiet("wpctl", &["set-volume", "@DEFAULT_AUDIO_SINK@", &v]);
}

pub fn toggle_mute() {
    run_quiet("wpctl", &["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"]);
}

pub fn volume_icon(level: f64, muted: bool) -> &'static str {
    if muted || level <= 0.001 {
        "audio-volume-muted-symbolic"
    } else if level < 0.34 {
        "audio-volume-low-symbolic"
    } else if level < 0.67 {
        "audio-volume-medium-symbolic"
    } else {
        "audio-volume-high-symbolic"
    }
}

// -------------------------------------------------------------- battery

pub struct Battery {
    pub percent: u8,
    pub charging: bool,
}

pub fn battery() -> Option<Battery> {
    for entry in fs::read_dir("/sys/class/power_supply").ok()?.flatten() {
        let p = entry.path();
        let kind = fs::read_to_string(p.join("type")).unwrap_or_default();
        if kind.trim() != "Battery" {
            continue;
        }
        let percent = fs::read_to_string(p.join("capacity"))
            .ok()?
            .trim()
            .parse::<u8>()
            .ok()?;
        let status = fs::read_to_string(p.join("status")).unwrap_or_default();
        let charging = matches!(status.trim(), "Charging" | "Full");
        return Some(Battery { percent, charging });
    }
    None
}

pub fn battery_icon(b: &Battery) -> String {
    let level = match b.percent {
        0..=10 => "empty",
        11..=30 => "caution",
        31..=60 => "low",
        61..=90 => "good",
        _ => "full",
    };
    if b.charging {
        format!("battery-{level}-charging-symbolic")
    } else {
        format!("battery-{level}-symbolic")
    }
}

// -------------------------------------------------------------- network

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Net {
    Wired,
    Wifi,
    Offline,
}

pub fn network() -> Net {
    let Ok(dir) = fs::read_dir("/sys/class/net") else {
        return Net::Offline;
    };
    let mut state = Net::Offline;
    for entry in dir.flatten() {
        let name = entry.file_name();
        if name == "lo" {
            continue;
        }
        let p = entry.path();
        let up = fs::read_to_string(p.join("operstate"))
            .map(|s| s.trim() == "up")
            .unwrap_or(false);
        if !up {
            continue;
        }
        if p.join("wireless").exists() {
            state = Net::Wifi;
        } else if state == Net::Offline {
            state = Net::Wired;
        }
    }
    state
}

pub fn network_icon(n: Net) -> &'static str {
    match n {
        Net::Wired => "network-wired-symbolic",
        Net::Wifi => "network-wireless-signal-excellent-symbolic",
        Net::Offline => "network-offline-symbolic",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_wpctl() {
        assert_eq!(parse_wpctl("Volume: 0.40\n"), Some((0.40, false)));
        assert_eq!(parse_wpctl("Volume: 1.00 [MUTED]\n"), Some((1.0, true)));
        assert_eq!(parse_wpctl("garbage"), None);
    }
}
