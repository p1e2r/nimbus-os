//! Full-screen app launcher. Running `nimbus-shell launcher` while it is
//! open closes it, so the same command works as a Super-key toggle.

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use std::rc::Rc;

struct Entry {
    child: gtk::FlowBoxChild,
    haystack: String,
    info: gio::AppInfo,
}

pub fn activate(app: &gtk::Application) {
    if let Some(win) = app.windows().first() {
        if win.is_visible() {
            win.set_visible(false);
        } else {
            reset(win);
            win.present();
        }
        return;
    }
    build(app);
}

fn reset(win: &gtk::Window) {
    // The search entry is stored on the window so a toggle can clear it.
    if let Some(entry) = unsafe { win.data::<gtk::SearchEntry>("search") } {
        let entry = unsafe { entry.as_ref() };
        entry.set_text("");
        entry.grab_focus();
    }
}

fn build(app: &gtk::Application) {
    let win = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Nimbus Launcher")
        .css_classes(["nimbus-overlay"])
        .build();
    win.init_layer_shell();
    win.set_namespace(Some("nimbus-launcher"));
    win.set_layer(Layer::Overlay);
    for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
        win.set_anchor(edge, true);
    }
    win.set_exclusive_zone(-1);
    win.set_keyboard_mode(KeyboardMode::Exclusive);

    let card = gtk::Box::new(gtk::Orientation::Vertical, 12);
    card.add_css_class("launcher-card");
    card.set_halign(gtk::Align::Center);
    card.set_valign(gtk::Align::Center);
    card.set_size_request(720, 520);

    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Search apps…"));
    search.add_css_class("launcher-search");
    card.append(&search);

    let flow = gtk::FlowBox::new();
    flow.set_selection_mode(gtk::SelectionMode::Single);
    flow.set_activate_on_single_click(true);
    flow.set_homogeneous(true);
    flow.set_min_children_per_line(4);
    flow.set_max_children_per_line(6);
    flow.set_valign(gtk::Align::Start);
    flow.set_row_spacing(6);
    flow.set_column_spacing(6);

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&flow)
        .build();
    card.append(&scroller);

    let entries: Rc<Vec<Entry>> = Rc::new(load_apps(&flow));

    // Filter as you type.
    {
        let entries = entries.clone();
        search.connect_search_changed(move |s| {
            let q = s.text().to_lowercase();
            for e in entries.iter() {
                e.child.set_visible(q.is_empty() || q.split_whitespace().all(|w| e.haystack.contains(w)));
            }
        });
    }

    // Enter launches the first match.
    {
        let entries = entries.clone();
        let win = win.clone();
        search.connect_activate(move |_| {
            if let Some(e) = entries.iter().find(|e| e.child.is_visible()) {
                launch(&win, &e.info);
            }
        });
    }

    // Click / keyboard activation of a tile.
    {
        let entries = entries.clone();
        let win = win.clone();
        flow.connect_child_activated(move |_, child| {
            if let Some(e) = entries.iter().find(|e| &e.child == child) {
                launch(&win, &e.info);
            }
        });
    }

    // Down arrow from the search box moves into the grid.
    {
        let flow = flow.clone();
        let entries = entries.clone();
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Down {
                if let Some(e) = entries.iter().find(|e| e.child.is_visible()) {
                    flow.select_child(&e.child);
                    e.child.grab_focus();
                    return glib::Propagation::Stop;
                }
            }
            glib::Propagation::Proceed
        });
        search.add_controller(keys);
    }

    // Escape closes.
    {
        let keys = gtk::EventControllerKey::new();
        let w = win.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                w.set_visible(false);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        win.add_controller(keys);
    }

    // Clicking the dimmed background closes; clicks on the card don't.
    let backdrop = gtk::Box::new(gtk::Orientation::Vertical, 0);
    backdrop.set_hexpand(true);
    backdrop.set_vexpand(true);
    backdrop.append(&card);
    {
        let click = gtk::GestureClick::new();
        let w = win.clone();
        let card = card.clone();
        click.connect_released(move |g, _, x, y| {
            let Some(widget) = g.widget() else { return };
            let inside = widget
                .compute_point(&card, &gtk::graphene::Point::new(x as f32, y as f32))
                .map(|p| card.contains(p.x() as f64, p.y() as f64))
                .unwrap_or(false);
            if !inside {
                w.set_visible(false);
            }
        });
        backdrop.add_controller(click);
    }

    win.set_child(Some(&backdrop));
    unsafe { win.set_data("search", search.clone()) };
    win.present();
    search.grab_focus();
}

fn load_apps(flow: &gtk::FlowBox) -> Vec<Entry> {
    let mut apps: Vec<gio::AppInfo> = gio::AppInfo::all()
        .into_iter()
        .filter(|a| a.should_show())
        .collect();
    apps.sort_by_key(|a| a.display_name().to_lowercase());
    apps.dedup_by_key(|a| a.id().map(|s| s.to_string()));

    apps.into_iter()
        .map(|info| {
            let tile = gtk::Box::new(gtk::Orientation::Vertical, 6);
            tile.add_css_class("app-tile");
            let img = match info.icon() {
                Some(icon) => gtk::Image::from_gicon(&icon),
                None => gtk::Image::from_icon_name("application-x-executable"),
            };
            img.set_pixel_size(48);
            let name = gtk::Label::new(Some(info.display_name().as_str()));
            name.set_ellipsize(gtk::pango::EllipsizeMode::End);
            name.set_max_width_chars(14);
            name.set_justify(gtk::Justification::Center);
            tile.append(&img);
            tile.append(&name);
            if let Some(desc) = info.description() {
                tile.set_tooltip_text(Some(desc.as_str()));
            }

            let child = gtk::FlowBoxChild::new();
            child.set_child(Some(&tile));
            flow.append(&child);

            let mut haystack = info.display_name().to_lowercase();
            if let Some(d) = info.description() {
                haystack.push(' ');
                haystack.push_str(&d.to_lowercase());
            }
            haystack.push(' ');
            if let Some(exe) = info.executable() {
                haystack.push_str(&exe.to_string_lossy().to_lowercase());
            }
            if let Some(desktop) = info.downcast_ref::<gio_unix::DesktopAppInfo>() {
                for k in desktop.keywords() {
                    haystack.push(' ');
                    haystack.push_str(&k.to_lowercase());
                }
            }
            Entry { child, haystack, info }
        })
        .collect()
}

fn launch(win: &gtk::ApplicationWindow, info: &gio::AppInfo) {
    let ctx = WidgetExt::display(win).app_launch_context();
    if let Err(e) = info.launch(&[], Some(&ctx)) {
        eprintln!("nimbus-shell: launch failed: {e}");
    }
    win.set_visible(false);
}
