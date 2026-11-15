//! The Wayland side: captures the screen with wlr-screencopy and injects
//! keyboard/mouse input with the wlr virtual-pointer and zwp
//! virtual-keyboard protocols. Runs on its own thread with its own
//! connection; talks to the RDP side through channels.

use std::fs::File;
use std::os::fd::{AsFd, AsRawFd, FromRawFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use memmap2::MmapMut;
use wayland_client::protocol::{
    wl_buffer, wl_output, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool,
};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1,
    zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
    zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
};
use xkbcommon::xkb;

/// A captured frame: BGRX/BGRA bytes, `stride` bytes per row.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    /// True when the bytes are R,G,B,X instead of B,G,R,X.
    pub rgb_order: bool,
    pub data: Vec<u8>,
}

/// Input from the RDP client, already translated to Linux codes.
#[derive(Debug)]
pub enum Input {
    /// Absolute position in screen pixels.
    Motion { x: u32, y: u32 },
    /// Linux button code (BTN_LEFT = 0x110 ...).
    Button { code: u32, pressed: bool },
    /// Scroll in "wheel clicks"; positive = down / right.
    Scroll { vertical: bool, clicks: f64 },
    /// Linux evdev key code.
    Key { code: u32, pressed: bool },
}

pub struct Handles {
    pub frames: mpsc::Receiver<Frame>,
    pub input: mpsc::Sender<Input>,
    /// Set while a client wants frames.
    pub capturing: Arc<AtomicBool>,
}

pub fn spawn(fps: u32) -> Result<Handles> {
    let (frame_tx, frame_rx) = mpsc::sync_channel(1);
    let (input_tx, input_rx) = mpsc::channel();
    let capturing = Arc::new(AtomicBool::new(false));
    let flag = capturing.clone();
    let conn = Connection::connect_to_env().context("connect to the Wayland compositor")?;
    std::thread::Builder::new()
        .name("wayland".into())
        .spawn(move || {
            if let Err(e) = run(conn, fps, frame_tx, input_rx, flag) {
                tracing::error!("wayland thread stopped: {e:#}");
                std::process::exit(1);
            }
        })?;
    Ok(Handles {
        frames: frame_rx,
        input: input_tx,
        capturing,
    })
}

#[derive(Default)]
struct Globals {
    shm: Option<wl_shm::WlShm>,
    output: Option<wl_output::WlOutput>,
    seat: Option<wl_seat::WlSeat>,
    screencopy: Option<ZwlrScreencopyManagerV1>,
    pointer_mgr: Option<ZwlrVirtualPointerManagerV1>,
    keyboard_mgr: Option<ZwpVirtualKeyboardManagerV1>,
}

struct ShmBuffer {
    buffer: wl_buffer::WlBuffer,
    map: MmapMut,
    width: u32,
    height: u32,
    stride: u32,
    format: wl_shm::Format,
}

#[derive(Default)]
struct PendingFrame {
    format: Option<wl_shm::Format>,
    width: u32,
    height: u32,
    stride: u32,
    buffer_done: bool,
    copied: bool,
    ready: bool,
    failed: bool,
    y_invert: bool,
}

struct State {
    globals: Globals,
    pending: Option<PendingFrame>,
    screencopy_version: u32,
}

fn run(
    conn: Connection,
    fps: u32,
    frames: mpsc::SyncSender<Frame>,
    input: mpsc::Receiver<Input>,
    capturing: Arc<AtomicBool>,
) -> Result<()> {
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());

    let mut state = State {
        globals: Globals::default(),
        pending: None,
        screencopy_version: 0,
    };
    queue.roundtrip(&mut state)?;

    let g = &state.globals;
    let shm = g.shm.clone().ok_or_else(|| anyhow!("compositor has no wl_shm"))?;
    let output = g.output.clone().ok_or_else(|| anyhow!("no outputs"))?;
    let screencopy = g
        .screencopy
        .clone()
        .ok_or_else(|| anyhow!("compositor does not support wlr-screencopy"))?;
    let seat = g.seat.clone();

    // Input devices (optional: view-only works without them).
    let pointer = match (&g.pointer_mgr, &seat) {
        (Some(m), seat) => Some(m.create_virtual_pointer(seat.as_ref(), &qh, ())),
        _ => None,
    };
    let mut keyboard = match (&g.keyboard_mgr, &seat) {
        (Some(m), Some(seat)) => Some(Keyboard::new(m.create_virtual_keyboard(seat, &qh, ()))?),
        _ => None,
    };
    if pointer.is_none() {
        tracing::warn!("no virtual pointer support: mouse input disabled");
    }
    if keyboard.is_none() {
        tracing::warn!("no virtual keyboard support: keyboard input disabled");
    }

    let frame_interval = Duration::from_millis(1000 / u64::from(fps.clamp(1, 60)));
    let start = Instant::now();
    let mut next_capture = Instant::now();
    let mut buffer: Option<ShmBuffer> = None;
    let mut in_flight: Option<ZwlrScreencopyFrameV1> = None;
    let mut screen = (0u32, 0u32);

    loop {
        queue.dispatch_pending(&mut state)?;

        // ---- screen capture state machine
        if let (Some(frame), Some(p)) = (&in_flight, &mut state.pending) {
            let wants_buffer = if state.screencopy_version >= 3 { p.buffer_done } else { p.format.is_some() };
            if wants_buffer && !p.copied && !p.failed {
                let format = p.format.ok_or_else(|| anyhow!("no usable shm format offered"))?;
                let reuse = buffer.as_ref().is_some_and(|b| {
                    b.width == p.width && b.height == p.height && b.stride == p.stride && b.format == format
                });
                if !reuse {
                    if let Some(old) = buffer.take() {
                        old.buffer.destroy();
                    }
                    buffer = Some(ShmBuffer::new(&shm, &qh, p.width, p.height, p.stride, format)?);
                }
                frame.copy(&buffer.as_ref().expect("buffer").buffer);
                p.copied = true;
            }
            if p.ready || p.failed {
                let p = state.pending.take().expect("pending");
                in_flight.take().expect("frame").destroy();
                if p.ready {
                    if let Some(b) = &buffer {
                        screen = (b.width, b.height);
                        let data = if p.y_invert { flip_rows(&b.map, b.stride, b.height) } else { b.map.to_vec() };
                        let rgb_order = matches!(b.format, wl_shm::Format::Xbgr8888 | wl_shm::Format::Abgr8888);
                        // Drop the frame if the RDP side is still busy with the previous one.
                        let _ = frames.try_send(Frame {
                            width: b.width,
                            height: b.height,
                            stride: b.stride,
                            rgb_order,
                            data,
                        });
                    }
                } else {
                    tracing::debug!("screencopy frame failed; retrying");
                }
            }
        }
        if in_flight.is_none() && capturing.load(Ordering::Relaxed) && Instant::now() >= next_capture {
            next_capture = Instant::now() + frame_interval;
            state.pending = Some(PendingFrame::default());
            in_flight = Some(screencopy.capture_output(1, &output, &qh, ()));
        }

        // ---- input from the client
        while let Ok(ev) = input.try_recv() {
            let time = start.elapsed().as_millis() as u32;
            match ev {
                Input::Motion { x, y } => {
                    if let Some(p) = &pointer {
                        if screen.0 > 0 {
                            p.motion_absolute(time, x.min(screen.0 - 1), y.min(screen.1 - 1), screen.0, screen.1);
                            p.frame();
                        }
                    }
                }
                Input::Button { code, pressed } => {
                    if let Some(p) = &pointer {
                        let st = if pressed { wl_pointer::ButtonState::Pressed } else { wl_pointer::ButtonState::Released };
                        p.button(time, code, st);
                        p.frame();
                    }
                }
                Input::Scroll { vertical, clicks } => {
                    if let Some(p) = &pointer {
                        let axis = if vertical { wl_pointer::Axis::VerticalScroll } else { wl_pointer::Axis::HorizontalScroll };
                        p.axis_source(wl_pointer::AxisSource::Wheel);
                        p.axis_discrete(time, axis, clicks * 15.0, clicks.round() as i32);
                        p.frame();
                    }
                }
                Input::Key { code, pressed } => {
                    if let Some(k) = &mut keyboard {
                        k.key(time, code, pressed);
                    }
                }
            }
        }

        conn.flush()?;

        // Sleep until Wayland has something for us, input arrives (checked
        // every few ms) or the next frame is due.
        if let Some(guard) = queue.prepare_read() {
            let timeout = if in_flight.is_some() { 5 } else { 10 };
            let mut pfd = libc::pollfd {
                fd: guard.connection_fd().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let n = unsafe { libc::poll(&mut pfd, 1, timeout) };
            if n > 0 {
                guard.read()?;
            }
        }
    }
}

fn flip_rows(src: &[u8], stride: u32, height: u32) -> Vec<u8> {
    let stride = stride as usize;
    let mut out = Vec::with_capacity(src.len());
    for row in (0..height as usize).rev() {
        out.extend_from_slice(&src[row * stride..(row + 1) * stride]);
    }
    out
}

fn memfd(name: &str, size: usize) -> Result<File> {
    let cname = std::ffi::CString::new(name)?;
    let fd = unsafe { libc::memfd_create(cname.as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error()).context("memfd_create");
    }
    let file = unsafe { File::from_raw_fd(fd) };
    file.set_len(size as u64)?;
    Ok(file)
}

impl ShmBuffer {
    fn new(
        shm: &wl_shm::WlShm,
        qh: &QueueHandle<State>,
        width: u32,
        height: u32,
        stride: u32,
        format: wl_shm::Format,
    ) -> Result<Self> {
        let size = (stride * height) as usize;
        let file = memfd("nimbus-rdp-frame", size)?;
        let map = unsafe { MmapMut::map_mut(&file)? };
        let pool = shm.create_pool(file.as_fd(), size as i32, qh, ());
        let buffer = pool.create_buffer(0, width as i32, height as i32, stride as i32, format, qh, ());
        pool.destroy();
        Ok(Self {
            buffer,
            map,
            width,
            height,
            stride,
            format,
        })
    }
}

// ------------------------------------------------------------- keyboard

struct Keyboard {
    vk: ZwpVirtualKeyboardV1,
    state: xkb::State,
    last_mods: (u32, u32, u32, u32),
}

impl Keyboard {
    fn new(vk: ZwpVirtualKeyboardV1) -> Result<Self> {
        let ctx = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        // Empty names = the system default layout (or $XKB_DEFAULT_LAYOUT).
        let keymap = xkb::Keymap::new_from_names(&ctx, "", "", "", "", None, xkb::KEYMAP_COMPILE_NO_FLAGS)
            .ok_or_else(|| anyhow!("could not compile an xkb keymap"))?;
        let text = keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
        let bytes = text.as_bytes();
        let file = memfd("nimbus-rdp-keymap", bytes.len() + 1)?;
        {
            let mut map = unsafe { MmapMut::map_mut(&file)? };
            map[..bytes.len()].copy_from_slice(bytes);
            map[bytes.len()] = 0;
        }
        vk.keymap(1 /* XKB_V1 */, file.as_fd(), (bytes.len() + 1) as u32);
        Ok(Self {
            vk,
            state: xkb::State::new(&keymap),
            last_mods: (0, 0, 0, 0),
        })
    }

    fn key(&mut self, time: u32, code: u32, pressed: bool) {
        self.vk.key(time, code, u32::from(pressed));
        let dir = if pressed { xkb::KeyDirection::Down } else { xkb::KeyDirection::Up };
        self.state.update_key(xkb::Keycode::new(code + 8), dir);
        let mods = (
            self.state.serialize_mods(xkb::STATE_MODS_DEPRESSED),
            self.state.serialize_mods(xkb::STATE_MODS_LATCHED),
            self.state.serialize_mods(xkb::STATE_MODS_LOCKED),
            self.state.serialize_layout(xkb::STATE_LAYOUT_EFFECTIVE),
        );
        if mods != self.last_mods {
            self.vk.modifiers(mods.0, mods.1, mods.2, mods.3);
            self.last_mods = mods;
        }
    }
}

// ------------------------------------------------------------ dispatch

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
            let g = &mut state.globals;
            match interface.as_str() {
                "wl_shm" => g.shm = Some(registry.bind(name, 1, qh, ())),
                "wl_output" if g.output.is_none() => g.output = Some(registry.bind(name, version.min(3), qh, ())),
                "wl_seat" if g.seat.is_none() => g.seat = Some(registry.bind(name, version.min(5), qh, ())),
                "zwlr_screencopy_manager_v1" => {
                    let v = version.min(3);
                    state.screencopy_version = v;
                    g.screencopy = Some(registry.bind(name, v, qh, ()));
                }
                "zwlr_virtual_pointer_manager_v1" => {
                    g.pointer_mgr = Some(registry.bind(name, version.min(2), qh, ()))
                }
                "zwp_virtual_keyboard_manager_v1" => g.keyboard_mgr = Some(registry.bind(name, 1, qh, ())),
                _ => {}
            }
        }
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(p) = state.pending.as_mut() else { return };
        use zwlr_screencopy_frame_v1::Event as E;
        match event {
            E::Buffer { format: WEnum::Value(format), width, height, stride } => {
                // Prefer formats we can hand to RDP without converting.
                let usable = matches!(
                    format,
                    wl_shm::Format::Xrgb8888 | wl_shm::Format::Argb8888 | wl_shm::Format::Xbgr8888 | wl_shm::Format::Abgr8888
                );
                if usable && p.format.is_none() {
                    p.format = Some(format);
                    p.width = width;
                    p.height = height;
                    p.stride = stride;
                }
            }
            E::Flags { flags: WEnum::Value(flags) } => {
                p.y_invert = flags.contains(zwlr_screencopy_frame_v1::Flags::YInvert);
            }
            E::BufferDone => p.buffer_done = true,
            E::Ready { .. } => p.ready = true,
            E::Failed => p.failed = true,
            _ => {}
        }
    }
}

macro_rules! ignore_events {
    ($($t:ty),* $(,)?) => {$(
        impl Dispatch<$t, ()> for State {
            fn event(_: &mut Self, _: &$t, _: <$t as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}

ignore_events!(
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_buffer::WlBuffer,
    wl_output::WlOutput,
    wl_seat::WlSeat,
    ZwlrScreencopyManagerV1,
    ZwlrVirtualPointerManagerV1,
    ZwlrVirtualPointerV1,
    ZwpVirtualKeyboardManagerV1,
    ZwpVirtualKeyboardV1,
);
