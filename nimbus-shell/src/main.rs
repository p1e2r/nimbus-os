//! Nimbus Desktop shell.
//!
//! One binary, several roles, picked by the first argument:
//!   nimbus-shell panel      top bar (default)
//!   nimbus-shell launcher   app launcher overlay (run again to toggle)
//!   nimbus-shell power      lock / log out / restart / shut down overlay
//!   nimbus-shell settings   settings window
//!   nimbus-shell wallpaper  start swaybg with the configured wallpaper

mod config;
mod launcher;
mod panel;
mod power;
mod settings;
mod sys;
mod taskbar;

use gtk::prelude::*;
use gtk::{gdk, glib};

const CSS: &str = include_str!("style.css");

fn main() -> glib::ExitCode {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "panel".to_string());

    let (app_id, activate): (&str, fn(&gtk::Application)) = match mode.as_str() {
        "panel" => ("org.nimbus.Panel", panel::activate),
        "launcher" => ("org.nimbus.Launcher", launcher::activate),
        "power" => ("org.nimbus.Power", power::activate),
        "settings" => ("org.nimbus.Settings", settings::activate),
        "wallpaper" => {
            sys::exec_wallpaper(&config::Config::load());
            return glib::ExitCode::FAILURE;
        }
        "--version" | "-V" => {
            println!("nimbus-shell {}", env!("CARGO_PKG_VERSION"));
            return glib::ExitCode::SUCCESS;
        }
        _ => {
            eprintln!("usage: nimbus-shell [panel|launcher|power|settings|wallpaper]");
            return glib::ExitCode::FAILURE;
        }
    };

    let app = gtk::Application::builder().application_id(app_id).build();
    app.connect_startup(|_| {
        let provider = gtk::CssProvider::new();
        provider.load_from_string(CSS);
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
        config::apply_color_scheme(config::Config::load().dark);
    });
    app.connect_activate(activate);
    // Don't let GTK parse our role argument.
    app.run_with_args(&["nimbus-shell"])
}
