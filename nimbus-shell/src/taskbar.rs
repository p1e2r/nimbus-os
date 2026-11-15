//! Window list for the panel, using the wlr-foreign-toplevel protocol.
//!
//! A background thread owns its own Wayland connection and forwards
//! window changes to the GTK main loop over a channel. The panel sends
//! back "activate" / "minimize" commands.

use std::collections::HashMap;
use std::os::fd::AsRawFd;
use std::sync::mpsc;

use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{event_created_child, Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1::{self as handle, ZwlrForeignToplevelHandleV1},
    zwlr_foreign_toplevel_manager_v1::{self as manager, ZwlrForeignToplevelManagerV1},
};

#[derive(Debug, Clone, Default)]
pub struct Window {
    pub title: String,
    pub app_id: String,
    pub active: bool,
    pub minimized: bool,
}

#[derive(Debug)]
pub enum Event {
    Changed(u32, Window),
    Closed(u32),
}

#[derive(Debug)]
pub enum Cmd {
    /// Raise and focus (un-minimizing if needed).
    Activate(u32),
    Minimize(u32),
    Close(u32),
}

pub fn start() -> (async_channel::Receiver<Event>, mpsc::Sender<Cmd>) {
    let (etx, erx) = async_channel::unbounded();
    let (ctx, crx) = mpsc::channel();
    std::thread::Builder::new()
        .name("nimbus-toplevels".into())
        .spawn(move || {
            if let Err(e) = run(etx, crx) {
                eprintln!("nimbus-shell: window list unavailable: {e}");
            }
        })
        .expect("spawn toplevel thread");
    (erx, ctx)
}

struct State {
    seat: Option<wl_seat::WlSeat>,
    manager: Option<ZwlrForeignToplevelManagerV1>,
    windows: HashMap<u32, (ZwlrForeignToplevelHandleV1, Window)>,
    tx: async_channel::Sender<Event>,
}

fn run(tx: async_channel::Sender<Event>, rx: mpsc::Receiver<Cmd>) -> Result<(), Box<dyn std::error::Error>> {
    let conn = Connection::connect_to_env()?;
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());

    let mut state = State {
        seat: None,
        manager: None,
        windows: HashMap::new(),
        tx,
    };
    queue.roundtrip(&mut state)?;
    if state.manager.is_none() {
        return Err("compositor does not support zwlr_foreign_toplevel_manager_v1".into());
    }

    loop {
        queue.dispatch_pending(&mut state)?;

        loop {
            match rx.try_recv() {
                Ok(cmd) => state.command(cmd),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
            }
        }
        if state.tx.is_closed() {
            return Ok(());
        }

        conn.flush()?;
        if let Some(guard) = queue.prepare_read() {
            let fd = guard.connection_fd().as_raw_fd();
            let mut pfd = libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            };
            // Wake up regularly so commands from the panel get handled.
            let ready = unsafe { libc::poll(&mut pfd, 1, 50) };
            if ready > 0 {
                guard.read()?;
            }
        }
    }
}

impl State {
    fn command(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Activate(id) => {
                if let (Some((h, _)), Some(seat)) = (self.windows.get(&id), &self.seat) {
                    h.unset_minimized();
                    h.activate(seat);
                }
            }
            Cmd::Minimize(id) => {
                if let Some((h, _)) = self.windows.get(&id) {
                    h.set_minimized();
                }
            }
            Cmd::Close(id) => {
                if let Some((h, _)) = self.windows.get(&id) {
                    h.close();
                }
            }
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global { name, interface, version } = event {
            match interface.as_str() {
                "wl_seat" if state.seat.is_none() => {
                    state.seat = Some(registry.bind(name, version.min(1), qh, ()));
                }
                "zwlr_foreign_toplevel_manager_v1" => {
                    state.manager = Some(registry.bind(name, version.min(3), qh, ()));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(_: &mut Self, _: &wl_seat::WlSeat, _: wl_seat::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrForeignToplevelManagerV1,
        event: manager::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let manager::Event::Toplevel { toplevel } = event {
            let id = toplevel.id().protocol_id();
            state.windows.insert(id, (toplevel, Window::default()));
        }
    }

    event_created_child!(State, ZwlrForeignToplevelManagerV1, [
        manager::EVT_TOPLEVEL_OPCODE => (ZwlrForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<ZwlrForeignToplevelHandleV1, ()> for State {
    fn event(
        state: &mut Self,
        h: &ZwlrForeignToplevelHandleV1,
        event: handle::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let id = h.id().protocol_id();
        match event {
            handle::Event::Closed => {
                state.windows.remove(&id);
                h.destroy();
                let _ = state.tx.send_blocking(Event::Closed(id));
            }
            handle::Event::Done => {
                if std::env::var_os("NIMBUS_DEBUG").is_some() { eprintln!("toplevel done {id} {:?}", state.windows.get(&id).map(|w| &w.1)); }
                if let Some((_, w)) = state.windows.get(&id) {
                    let _ = state.tx.send_blocking(Event::Changed(id, w.clone()));
                }
            }
            other => {
                let Some((_, w)) = state.windows.get_mut(&id) else { return };
                match other {
                    handle::Event::Title { title } => w.title = title,
                    handle::Event::AppId { app_id } => w.app_id = app_id,
                    handle::Event::State { state: raw } => {
                        let states: Vec<u32> = raw
                            .chunks_exact(4)
                            .map(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]]))
                            .collect();
                        w.active = states.contains(&(handle::State::Activated as u32));
                        w.minimized = states.contains(&(handle::State::Minimized as u32));
                    }
                    _ => {}
                }
            }
        }
    }
}
