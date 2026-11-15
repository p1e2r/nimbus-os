//! Nimbus Settings: appearance, wallpaper, pinned apps, shortcuts to
//! system tools, and an About page.

use crate::config::{self, Config};
use crate::sys;
use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::rc::Rc;

pub fn activate(app: &gtk::Application) {
    if let Some(win) = app.windows().first() {
        win.present();
        return;
    }

    let cfg = Rc::new(RefCell::new(Config::load()));

    let win = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Nimbus Settings")
        .default_width(760)
        .default_height(520)
        .build();

    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    stack.add_titled(&appearance_page(&win, &cfg), Some("appearance"), "Appearance");
    stack.add_titled(&panel_page(&cfg), Some("panel"), "Panel");
    stack.add_titled(&remote_page(), Some("remote"), "Remote desktop");
    stack.add_titled(&system_page(), Some("system"), "System");
    stack.add_titled(&about_page(), Some("about"), "About");

    let sidebar = gtk::StackSidebar::new();
    sidebar.set_stack(&stack);
    sidebar.set_width_request(180);

    let paned = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    paned.append(&sidebar);
    paned.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    stack.set_hexpand(true);
    paned.append(&stack);

    win.set_child(Some(&paned));
    win.present();
}

fn page() -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 14);
    b.add_css_class("settings-page");
    b.set_margin_top(24);
    b.set_margin_bottom(24);
    b.set_margin_start(28);
    b.set_margin_end(28);
    b
}

fn heading(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.add_css_class("title-2");
    l.set_halign(gtk::Align::Start);
    l
}

fn row(title: &str, subtitle: &str, widget: &impl IsA<gtk::Widget>) -> gtk::Box {
    let r = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    r.add_css_class("settings-row");
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    let t = gtk::Label::new(Some(title));
    t.set_halign(gtk::Align::Start);
    let s = gtk::Label::new(Some(subtitle));
    s.set_halign(gtk::Align::Start);
    s.add_css_class("dim-label");
    s.set_wrap(true);
    text.append(&t);
    text.append(&s);
    r.append(&text);
    widget.set_valign(gtk::Align::Center);
    r.append(widget);
    r
}

fn save(cfg: &Rc<RefCell<Config>>) {
    if let Err(e) = cfg.borrow().save() {
        eprintln!("nimbus-settings: could not save config: {e}");
    }
}

// -------------------------------------------------------------- appearance

fn appearance_page(win: &gtk::ApplicationWindow, cfg: &Rc<RefCell<Config>>) -> gtk::Box {
    let p = page();
    p.append(&heading("Appearance"));

    let dark = gtk::Switch::new();
    dark.set_active(cfg.borrow().dark);
    {
        let cfg = cfg.clone();
        dark.connect_active_notify(move |s| {
            cfg.borrow_mut().dark = s.is_active();
            save(&cfg);
            config::apply_color_scheme(s.is_active());
        });
    }
    p.append(&row("Dark style", "Use dark colours in apps that support it.", &dark));

    let preview = gtk::Picture::for_filename(&cfg.borrow().wallpaper);
    preview.set_content_fit(gtk::ContentFit::Cover);
    preview.set_size_request(320, 180);
    preview.add_css_class("wallpaper-preview");
    preview.set_halign(gtk::Align::Start);

    let choose = gtk::Button::with_label("Choose…");
    {
        let cfg = cfg.clone();
        let win = win.clone();
        let preview = preview.clone();
        choose.connect_clicked(move |_| {
            let filter = gtk::FileFilter::new();
            filter.set_name(Some("Images"));
            filter.add_mime_type("image/*");
            let filters = gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&filter);
            let dialog = gtk::FileDialog::builder()
                .title("Choose a wallpaper")
                .filters(&filters)
                .initial_folder(&gio::File::for_path("/usr/share/backgrounds"))
                .build();
            let cfg = cfg.clone();
            let preview = preview.clone();
            dialog.open(Some(&win), gio::Cancellable::NONE, move |res| {
                let Ok(file) = res else { return };
                let Some(path) = file.path() else { return };
                cfg.borrow_mut().wallpaper = path.to_string_lossy().into_owned();
                save(&cfg);
                preview.set_filename(Some(&path));
                sys::restart_wallpaper();
            });
        });
    }
    p.append(&row("Wallpaper", "Picture shown behind your windows.", &choose));
    p.append(&preview);
    p
}

// ------------------------------------------------------------------ panel

fn panel_page(cfg: &Rc<RefCell<Config>>) -> gtk::Box {
    let p = page();
    p.append(&heading("Panel"));

    let pinned = gtk::Entry::new();
    pinned.set_text(&cfg.borrow().pinned.join(", "));
    pinned.set_width_chars(36);
    p.append(&row(
        "Pinned apps",
        "Desktop file names, separated by commas (e.g. firefox.desktop).",
        &pinned,
    ));

    let clock = gtk::Entry::new();
    clock.set_text(&cfg.borrow().clock_format);
    clock.set_width_chars(20);
    p.append(&row("Clock format", "Uses strftime codes like %H:%M or %a %d %b.", &clock));

    let apply = gtk::Button::with_label("Apply and restart panel");
    apply.add_css_class("suggested-action");
    apply.set_halign(gtk::Align::End);
    {
        let cfg = cfg.clone();
        apply.connect_clicked(move |_| {
            {
                let mut c = cfg.borrow_mut();
                c.pinned = pinned
                    .text()
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                c.clock_format = clock.text().to_string();
            }
            save(&cfg);
            sys::run_quiet("pkill", &["-f", "nimbus-shell panel"]);
            sys::spawn_self("panel");
        });
    }
    p.append(&apply);
    p
}

// ---------------------------------------------------------- remote desktop

fn rdp_config_path() -> std::path::PathBuf {
    config::path().with_file_name("rdp.toml")
}

fn load_rdp() -> toml::Table {
    std::fs::read_to_string(rdp_config_path())
        .ok()
        .and_then(|s| s.parse::<toml::Table>().ok())
        .unwrap_or_default()
}

fn save_rdp(table: &toml::Table) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let path = rdp_config_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Holds a password: readable by this user only.
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)?;
    f.write_all(table.to_string().as_bytes())
}

fn rdp_running() -> bool {
    std::process::Command::new("pgrep")
        .args(["-x", "nimbus-rdp"])
        .stdout(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// IPv4 addresses other machines can use to reach this one.
fn local_addresses() -> Vec<String> {
    let out = std::process::Command::new("ip")
        .args(["-4", "-o", "addr", "show", "scope", "global"])
        .output();
    let Ok(out) = out else { return Vec::new() };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.split_whitespace().nth(3))
        .filter_map(|cidr| cidr.split('/').next())
        .map(str::to_string)
        .collect()
}

fn remote_page() -> gtk::Box {
    let p = page();
    p.append(&heading("Remote desktop"));

    let intro = gtk::Label::new(Some(
        "Let other computers see and control this screen over RDP. Use Windows App / Microsoft \
         Remote Desktop on Mac or iPhone, Remote Desktop Connection on Windows, or Remmina on Linux.",
    ));
    intro.set_wrap(true);
    intro.set_xalign(0.0);
    intro.add_css_class("dim-label");
    p.append(&intro);

    let table = load_rdp();
    let get_str = |k: &str, d: &str| table.get(k).and_then(|v| v.as_str()).unwrap_or(d).to_string();
    let user_default = std::env::var("USER").unwrap_or_else(|_| "nimbus".into());

    let enabled = gtk::Switch::new();
    enabled.set_active(table.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false));
    p.append(&row("Allow remote desktop", "Starts automatically when you log in.", &enabled));

    let username = gtk::Entry::new();
    username.set_text(&get_str("username", &user_default));
    username.set_width_chars(20);
    p.append(&row("Username", "The name to type in the RDP client.", &username));

    let password = gtk::PasswordEntry::new();
    password.set_show_peek_icon(true);
    password.set_text(&get_str("password", ""));
    password.set_width_chars(20);
    p.append(&row("Password", "Required. Pick something only you know.", &password));

    let port = gtk::SpinButton::with_range(1024.0, 65535.0, 1.0);
    port.set_value(table.get("port").and_then(|v| v.as_integer()).unwrap_or(3389) as f64);
    p.append(&row("Port", "3389 is the standard RDP port.", &port));

    let view_only = gtk::Switch::new();
    view_only.set_active(table.get("view_only").and_then(|v| v.as_bool()).unwrap_or(false));
    p.append(&row("View only", "Show the screen but ignore the remote keyboard and mouse.", &view_only));

    let status = gtk::Label::new(None);
    status.set_xalign(0.0);
    status.set_wrap(true);
    status.set_selectable(true);
    p.append(&status);

    let refresh_status = {
        let status = status.clone();
        let port = port.clone();
        move || {
            if rdp_running() {
                let addrs = local_addresses();
                let port = port.value() as u16;
                let target = match addrs.first() {
                    Some(a) if port == 3389 => a.clone(),
                    Some(a) => format!("{a}:{port}"),
                    None => format!("this computer's IP address (port {port})"),
                };
                status.set_markup(&format!(
                    "<b>On.</b> Connect to <b>{}</b>. The first time, the client will ask you to trust this computer's certificate.",
                    glib::markup_escape_text(&target)
                ));
            } else {
                status.set_markup("<b>Off.</b>");
            }
        }
    };
    refresh_status();

    let apply = gtk::Button::with_label("Apply");
    apply.add_css_class("suggested-action");
    apply.set_halign(gtk::Align::End);
    {
        let status = status.clone();
        apply.connect_clicked(move |_| {
            let pw = password.text().to_string();
            if enabled.is_active() && pw.is_empty() {
                status.set_markup("<b>Set a password first.</b> Remote desktop needs one.");
                return;
            }
            let mut t = load_rdp();
            t.insert("enabled".into(), enabled.is_active().into());
            t.insert("username".into(), username.text().trim().to_string().into());
            t.insert("password".into(), pw.into());
            t.insert("port".into(), (port.value() as i64).into());
            t.insert("view_only".into(), view_only.is_active().into());
            if let Err(e) = save_rdp(&t) {
                status.set_text(&format!("Could not save settings: {e}"));
                return;
            }
            sys::run_quiet("pkill", &["-x", "nimbus-rdp"]);
            if enabled.is_active() {
                sys::spawn("sleep 0.5; exec nimbus-rdp");
            }
            let refresh = refresh_status.clone();
            glib::timeout_add_seconds_local_once(2, refresh);
        });
    }
    p.append(&apply);
    p
}

// ----------------------------------------------------------------- system

fn system_page() -> gtk::Box {
    let p = page();
    p.append(&heading("System"));
    let tools = [
        ("Displays", "Resolution, scaling and arrangement.", "wdisplays"),
        ("Network", "Wi-Fi, Ethernet and VPN connections.", "nm-connection-editor"),
        ("Sound", "Output and input devices, per-app volume.", "pavucontrol"),
        ("Bluetooth", "Pair and manage Bluetooth devices.", "blueman-manager"),
        ("Software updates", "Update Arch and Nimbus packages in a terminal.", sys::UPDATE_COMMAND),
    ];
    for (title, sub, cmd) in tools {
        let b = gtk::Button::with_label("Open");
        let cmd = cmd.to_string();
        b.connect_clicked(move |_| sys::spawn(&cmd));
        p.append(&row(title, sub, &b));
    }
    p
}

// ------------------------------------------------------------------ about

fn about_page() -> gtk::Box {
    let p = page();
    p.set_halign(gtk::Align::Center);
    p.set_valign(gtk::Align::Center);

    let logo = gtk::Image::from_icon_name("nimbus-logo");
    logo.set_pixel_size(96);
    p.append(&logo);

    let name = gtk::Label::new(Some("Nimbus"));
    name.add_css_class("title-1");
    p.append(&name);

    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default();
    let os = os_pretty_name();
    let info = format!(
        "Nimbus Desktop {}\nBased on {os}\nLinux {}\nHost: {}",
        env!("CARGO_PKG_VERSION"),
        kernel.trim(),
        glib::host_name()
    );
    let l = gtk::Label::new(Some(&info));
    l.set_justify(gtk::Justification::Center);
    l.add_css_class("dim-label");
    p.append(&l);

    let link = gtk::LinkButton::with_label("https://github.com/p1e2r/nimbus-os", "Project page");
    p.append(&link);
    p
}

fn os_pretty_name() -> String {
    // /usr/lib/os-release is the upstream (Arch) one; /etc/os-release says Nimbus.
    std::fs::read_to_string("/usr/lib/os-release")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("PRETTY_NAME=").map(|v| v.trim_matches('"').to_string()))
        })
        .unwrap_or_else(|| "Arch Linux".into())
}
