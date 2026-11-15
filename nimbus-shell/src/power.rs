//! Power overlay: lock, log out, restart, shut down.

use crate::config::Config;
use crate::sys;
use gtk::prelude::*;
use gtk::{gdk, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

pub fn activate(app: &gtk::Application) {
    // Second invocation closes it again.
    if let Some(win) = app.windows().first() {
        win.close();
        return;
    }

    let win = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Nimbus Power")
        .css_classes(["nimbus-overlay"])
        .build();
    win.init_layer_shell();
    win.set_namespace(Some("nimbus-power"));
    win.set_layer(Layer::Overlay);
    for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
        win.set_anchor(edge, true);
    }
    win.set_exclusive_zone(-1);
    win.set_keyboard_mode(KeyboardMode::Exclusive);

    let col = gtk::Box::new(gtk::Orientation::Vertical, 24);
    col.set_halign(gtk::Align::Center);
    col.set_valign(gtk::Align::Center);

    let user = glib::real_name();
    let title = gtk::Label::new(Some(&format!("Goodbye, {}", user.to_string_lossy())));
    title.add_css_class("power-title");
    col.append(&title);

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    let lock_cmd = Config::load().lock_command;
    let actions: [(&str, &str, String); 4] = [
        ("Lock", "system-lock-screen-symbolic", lock_cmd),
        ("Log out", "system-log-out-symbolic", "labwc --exit".into()),
        ("Restart", "system-reboot-symbolic", "systemctl reboot".into()),
        ("Shut down", "system-shutdown-symbolic", "systemctl poweroff".into()),
    ];
    for (label, icon, cmd) in actions {
        let b = gtk::Button::new();
        b.add_css_class("power-action");
        let inner = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let img = gtk::Image::from_icon_name(icon);
        img.set_pixel_size(40);
        inner.append(&img);
        inner.append(&gtk::Label::new(Some(label)));
        b.set_child(Some(&inner));
        let w = win.clone();
        b.connect_clicked(move |_| {
            w.close();
            sys::spawn(&cmd);
        });
        row.append(&b);
    }
    col.append(&row);

    let cancel = gtk::Button::with_label("Cancel");
    cancel.add_css_class("flat");
    cancel.set_halign(gtk::Align::Center);
    {
        let w = win.clone();
        cancel.connect_clicked(move |_| w.close());
    }
    col.append(&cancel);

    let keys = gtk::EventControllerKey::new();
    {
        let w = win.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                w.close();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
    }
    win.add_controller(keys);

    win.set_child(Some(&col));
    win.present();
}
