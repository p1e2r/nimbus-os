//! The top bar: launcher button, pinned apps, clock, status icons, power.

use crate::config::Config;
use crate::sys;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use crate::taskbar::{self, Cmd};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::mpsc;

thread_local! {
    static WINDOWS: RefCell<BTreeMap<u32, taskbar::Window>> = const { RefCell::new(BTreeMap::new()) };
    static BARS: RefCell<Vec<glib::WeakRef<gtk::Box>>> = const { RefCell::new(Vec::new()) };
    static CMDS: RefCell<Option<mpsc::Sender<Cmd>>> = const { RefCell::new(None) };
}

const PANEL_HEIGHT: i32 = 36;

pub fn activate(app: &gtk::Application) {
    // A second `nimbus-shell panel` just re-activates the running one.
    if !app.windows().is_empty() {
        return;
    }
    let Some(display) = gdk::Display::default() else {
        eprintln!("nimbus-shell: no display");
        return;
    };

    start_window_list();

    let monitors = display.monitors();
    build_all(app, &monitors);

    // Rebuild when monitors are plugged or unplugged.
    let app_weak = app.downgrade();
    monitors.connect_items_changed(move |list, _, _, _| {
        if let Some(app) = app_weak.upgrade() {
            for w in app.windows() {
                w.destroy();
            }
            build_all(&app, list);
        }
    });

    // Keep running even if every monitor disappears for a moment.
    std::mem::forget(app.hold());
}

fn build_all(app: &gtk::Application, monitors: &gio::ListModel) {
    for i in 0..monitors.n_items() {
        if let Some(mon) = monitors.item(i).and_downcast::<gdk::Monitor>() {
            build_panel(app, &mon);
        }
    }
}

fn build_panel(app: &gtk::Application, monitor: &gdk::Monitor) {
    let cfg = Config::load();

    let win = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Nimbus Panel")
        .default_height(PANEL_HEIGHT)
        .css_classes(["nimbus-panel"])
        .build();

    win.init_layer_shell();
    win.set_namespace(Some("nimbus-panel"));
    win.set_layer(Layer::Top);
    win.set_monitor(Some(monitor));
    win.set_anchor(Edge::Top, true);
    win.set_anchor(Edge::Left, true);
    win.set_anchor(Edge::Right, true);
    win.set_keyboard_mode(KeyboardMode::OnDemand);
    win.auto_exclusive_zone_enable();

    let bar = gtk::CenterBox::new();
    bar.add_css_class("bar");
    bar.set_start_widget(Some(&left_section(&cfg)));
    bar.set_center_widget(Some(&clock(&cfg)));
    bar.set_end_widget(Some(&right_section()));

    win.set_child(Some(&bar));
    win.present();
}

// ------------------------------------------------------------- left side

fn left_section(cfg: &Config) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);

    let start = gtk::Button::new();
    start.add_css_class("start");
    let inner = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    inner.append(&gtk::Image::from_icon_name("nimbus-logo"));
    inner.append(&gtk::Label::new(Some("Nimbus")));
    start.set_child(Some(&inner));
    start.set_tooltip_text(Some("Applications (Super)"));
    start.connect_clicked(|_| sys::spawn_self("launcher"));
    row.append(&start);

    row.append(&gtk::Separator::new(gtk::Orientation::Vertical));

    let pins = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    row.append(&pins);
    for id in &cfg.pinned {
        let Some(info) = id
            .split('|')
            .find_map(|alt| gio_unix::DesktopAppInfo::new(alt.trim()))
        else {
            continue;
        };
        let btn = gtk::Button::new();
        btn.add_css_class("pinned");
        let img = match info.icon() {
            Some(icon) => gtk::Image::from_gicon(&icon),
            None => gtk::Image::from_icon_name("application-x-executable"),
        };
        img.set_pixel_size(20);
        btn.set_child(Some(&img));
        btn.set_tooltip_text(Some(info.display_name().as_str()));
        btn.connect_clicked(move |b| {
            let ctx = b.display().app_launch_context();
            if let Err(e) = info.launch(&[], Some(&ctx)) {
                eprintln!("nimbus-shell: launch failed: {e}");
            }
        });
        pins.append(&btn);
    }

    if pins.first_child().is_some() {
        row.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    }

    let tasks = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    tasks.add_css_class("tasks");
    render_tasks(&tasks);
    BARS.with(|b| b.borrow_mut().push(tasks.downgrade()));
    row.append(&tasks);
    row
}

// ----------------------------------------------------------- window list

fn start_window_list() {
    let (events, cmds) = taskbar::start();
    CMDS.with(|c| *c.borrow_mut() = Some(cmds));
    glib::spawn_future_local(async move {
        while let Ok(ev) = events.recv().await {
            WINDOWS.with(|w| {
                let mut w = w.borrow_mut();
                match ev {
                    taskbar::Event::Changed(id, info) => {
                        w.insert(id, info);
                    }
                    taskbar::Event::Closed(id) => {
                        w.remove(&id);
                    }
                }
            });
            BARS.with(|bars| {
                bars.borrow_mut().retain(|b| b.upgrade().is_some());
                for b in bars.borrow().iter().filter_map(|b| b.upgrade()) {
                    render_tasks(&b);
                }
            });
        }
    });
}

fn send(cmd: Cmd) {
    CMDS.with(|c| {
        if let Some(tx) = c.borrow().as_ref() {
            let _ = tx.send(cmd);
        }
    });
}

fn app_icon(app_id: &str) -> gtk::Image {
    let candidates = [app_id.to_string(), app_id.to_lowercase()];
    for c in &candidates {
        if let Some(info) = gio_unix::DesktopAppInfo::new(&format!("{c}.desktop")) {
            if let Some(icon) = info.icon() {
                return gtk::Image::from_gicon(&icon);
            }
        }
    }
    if let Some(display) = gdk::Display::default() {
        let theme = gtk::IconTheme::for_display(&display);
        for c in &candidates {
            if theme.has_icon(c) {
                return gtk::Image::from_icon_name(c);
            }
        }
    }
    gtk::Image::from_icon_name("application-x-executable")
}

fn render_tasks(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
    WINDOWS.with(|w| {
        for (&id, win) in w.borrow().iter() {
            let btn = gtk::Button::new();
            btn.add_css_class("task");
            if win.active && !win.minimized {
                btn.add_css_class("active");
            }
            if win.minimized {
                btn.add_css_class("minimized");
            }
            let inner = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            let img = app_icon(&win.app_id);
            img.set_pixel_size(18);
            inner.append(&img);
            let title = if win.title.is_empty() { &win.app_id } else { &win.title };
            let label = gtk::Label::new(Some(title));
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            label.set_max_width_chars(18);
            inner.append(&label);
            btn.set_child(Some(&inner));
            btn.set_tooltip_text(Some(title));

            let focused = win.active && !win.minimized;
            btn.connect_clicked(move |_| {
                send(if focused { Cmd::Minimize(id) } else { Cmd::Activate(id) });
            });
            let middle = gtk::GestureClick::builder().button(2).build();
            middle.connect_released(move |_, _, _, _| send(Cmd::Close(id)));
            btn.add_controller(middle);

            container.append(&btn);
        }
    });
}

// ----------------------------------------------------------------- clock

fn clock(cfg: &Config) -> gtk::MenuButton {
    let label = gtk::Label::new(None);
    label.add_css_class("clock");

    let fmt = cfg.clock_format.clone();
    let update = {
        let label = label.clone();
        move || {
            if let Ok(now) = glib::DateTime::now_local() {
                if let Ok(text) = now.format(&fmt) {
                    label.set_text(&text);
                }
            }
        }
    };
    update();
    glib::timeout_add_seconds_local(1, move || {
        update();
        glib::ControlFlow::Continue
    });

    let popover = gtk::Popover::new();
    let calendar = gtk::Calendar::new();
    popover.set_child(Some(&calendar));
    popover.connect_show(move |_| {
        if let Ok(now) = glib::DateTime::now_local() {
            calendar.select_day(&now);
        }
    });

    let button = gtk::MenuButton::new();
    button.add_css_class("flat");
    button.set_child(Some(&label));
    button.set_popover(Some(&popover));
    button
}

// ------------------------------------------------------------ right side

fn right_section() -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    row.add_css_class("status");

    // Network
    let net_btn = gtk::Button::from_icon_name(sys::network_icon(sys::network()));
    net_btn.add_css_class("flat");
    net_btn.set_tooltip_text(Some("Network settings"));
    net_btn.connect_clicked(|_| sys::spawn("nm-connection-editor"));
    row.append(&net_btn);

    // Volume
    let (vol_btn, refresh_volume) = volume_button();
    row.append(&vol_btn);

    // Battery (only shown on machines that have one)
    let bat_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let bat_icon = gtk::Image::new();
    let bat_label = gtk::Label::new(None);
    bat_box.append(&bat_icon);
    bat_box.append(&bat_label);
    bat_box.add_css_class("battery");
    row.append(&bat_box);

    // Settings + power
    let settings = gtk::Button::from_icon_name("emblem-system-symbolic");
    settings.add_css_class("flat");
    settings.set_tooltip_text(Some("Settings"));
    settings.connect_clicked(|_| sys::spawn_self("settings"));
    row.append(&settings);

    let power = gtk::Button::from_icon_name("system-shutdown-symbolic");
    power.add_css_class("flat");
    power.add_css_class("power");
    power.set_tooltip_text(Some("Power"));
    power.connect_clicked(|_| sys::spawn_self("power"));
    row.append(&power);

    let tick = move || {
        net_btn.set_icon_name(sys::network_icon(sys::network()));
        refresh_volume();
        match sys::battery() {
            Some(b) => {
                bat_icon.set_icon_name(Some(&sys::battery_icon(&b)));
                bat_label.set_text(&format!("{}%", b.percent));
                bat_box.set_visible(true);
            }
            None => bat_box.set_visible(false),
        }
    };
    tick();
    glib::timeout_add_seconds_local(2, move || {
        tick();
        glib::ControlFlow::Continue
    });

    row
}

fn volume_button() -> (gtk::MenuButton, impl Fn()) {
    let icon = gtk::Image::from_icon_name("audio-volume-medium-symbolic");
    let button = gtk::MenuButton::new();
    button.add_css_class("flat");
    button.set_child(Some(&icon));
    button.set_tooltip_text(Some("Volume"));

    let popover = gtk::Popover::new();
    let col = gtk::Box::new(gtk::Orientation::Vertical, 8);
    col.add_css_class("popover-body");
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
    scale.set_width_request(220);
    scale.set_draw_value(true);
    scale.set_value_pos(gtk::PositionType::Right);
    let mute = gtk::ToggleButton::with_label("Mute");
    let mixer = gtk::Button::with_label("Sound settings…");
    mixer.connect_clicked(|_| sys::spawn("pavucontrol"));
    col.append(&scale);
    col.append(&mute);
    col.append(&mixer);
    popover.set_child(Some(&col));
    button.set_popover(Some(&popover));

    // Ignore our own programmatic updates.
    let syncing = Rc::new(Cell::new(false));

    {
        let syncing = syncing.clone();
        scale.connect_value_changed(move |s| {
            if !syncing.get() {
                sys::set_volume(s.value() / 100.0);
            }
        });
    }
    {
        let syncing = syncing.clone();
        mute.connect_toggled(move |_| {
            if !syncing.get() {
                sys::toggle_mute();
            }
        });
    }

    // Scroll on the icon to change volume.
    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    scroll.connect_scroll(|_, _, dy| {
        if let Some((v, _)) = sys::volume() {
            sys::set_volume(v - dy * 0.05);
        }
        glib::Propagation::Stop
    });
    button.add_controller(scroll);

    let target = button.clone();
    let refresh = move || match sys::volume() {
        Some((level, muted)) => {
            icon.set_icon_name(Some(sys::volume_icon(level, muted)));
            target.set_tooltip_text(Some(&format!("Volume {:.0}%", level * 100.0)));
            syncing.set(true);
            if !popover.is_visible() {
                scale.set_value(level * 100.0);
            }
            mute.set_active(muted);
            syncing.set(false);
        }
        None => icon.set_icon_name(Some("audio-volume-muted-symbolic")),
    };
    (button, refresh)
}
