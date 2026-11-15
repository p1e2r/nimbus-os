//! nimbus-rdp: share the Nimbus Desktop screen over RDP.
//!
//!   nimbus-rdp               run the server with ~/.config/nimbus/rdp.toml
//!   nimbus-rdp --if-enabled  same, but exit quietly unless enabled there
//!
//! Connect with Microsoft Remote Desktop / Windows App, mstsc, Remmina or
//! FreeRDP, using the username and password set in Nimbus Settings.

mod config;
mod keymap;
mod wayland;

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use bytes::Bytes;
use core::num::{NonZeroU16, NonZeroUsize};
use ironrdp_server::{
    BitmapUpdate, Credentials, DesktopSize, DisplayUpdate, KeyboardEvent, MouseEvent, PixelFormat, RdpServer,
    RdpServerDisplay, RdpServerDisplayUpdates, RdpServerInputHandler, TlsIdentityCtx,
};
use tokio::sync::{mpsc as tmpsc, Mutex, OwnedMutexGuard};

use wayland::{Frame, Input};

const TILE: usize = 64;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("NIMBUS_RDP_LOG").unwrap_or_else(|_| "nimbus_rdp=info,warn".into()),
        )
        .init();

    let if_enabled = std::env::args().any(|a| a == "--if-enabled");
    let cfg = config::Config::load();
    if if_enabled && !cfg.enabled {
        return Ok(());
    }
    if cfg.password.is_empty() {
        bail!("no password set: choose one in Nimbus Settings → Remote desktop ({})", config::Config::path().display());
    }

    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    rt.block_on(serve(cfg))
}

async fn serve(cfg: config::Config) -> Result<()> {
    let tls = tls_identity()?;
    let acceptor = tls.make_acceptor()?;

    let wl = wayland::spawn(cfg.fps)?;

    // Bridge the Wayland thread's frames into the async world.
    let (frame_tx, frame_rx) = tmpsc::channel::<Frame>(1);
    let frames_std = wl.frames;
    std::thread::Builder::new().name("frames".into()).spawn(move || {
        while let Ok(f) = frames_std.recv() {
            if frame_tx.blocking_send(f).is_err() {
                break;
            }
        }
    })?;
    let frames = Arc::new(Mutex::new(frame_rx));

    // Grab one frame now so we know the screen size before a client asks.
    wl.capturing.store(true, Ordering::Relaxed);
    let first = frames.lock().await.recv().await.context("no frame from the compositor")?;
    wl.capturing.store(false, Ordering::Relaxed);
    let size = DesktopSize {
        width: first.width as u16,
        height: first.height as u16,
    };
    tracing::info!("screen is {}x{}", size.width, size.height);

    let display = Display {
        frames,
        capturing: wl.capturing.clone(),
        size,
    };

    // Dual-stack when IPv6 is available, plain IPv4 otherwise.
    let v6 = SocketAddr::from((Ipv6Addr::UNSPECIFIED, cfg.port));
    let addr = match std::net::TcpListener::bind(v6) {
        Ok(probe) => {
            drop(probe);
            v6
        }
        Err(_) => SocketAddr::from((Ipv4Addr::UNSPECIFIED, cfg.port)),
    };
    let builder = RdpServer::builder().with_addr(addr).with_hybrid(acceptor, tls.pub_key.clone());
    let mut server = if cfg.view_only {
        builder.with_no_input().with_display_handler(display).build()
    } else {
        builder
            .with_input_handler(InputHandler { tx: wl.input })
            .with_display_handler(display)
            .build()
    };
    server.set_credentials(Some(Credentials {
        username: cfg.username.clone(),
        password: cfg.password.clone(),
        domain: None,
    }));

    tracing::info!("listening on port {} as user {:?}", cfg.port, cfg.username);
    server.run().await
}

/// Self-signed certificate, created once per user.
fn tls_identity() -> Result<TlsIdentityCtx> {
    let dir = config::data_dir();
    let cert = dir.join("cert.pem");
    let key = dir.join("key.pem");
    if !cert.exists() || !key.exists() {
        std::fs::create_dir_all(&dir)?;
        let host = std::fs::read_to_string("/etc/hostname").unwrap_or_else(|_| "nimbus".into());
        let generated = rcgen::generate_simple_self_signed(vec![host.trim().to_string(), "localhost".into()])?;
        write_private(&cert, generated.cert.pem().as_bytes())?;
        write_private(&key, generated.signing_key.serialize_pem().as_bytes())?;
        tracing::info!("created TLS certificate in {}", dir.display());
    }
    TlsIdentityCtx::init_from_paths(&cert, &key)
}

fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(data)?;
    Ok(())
}

// ----------------------------------------------------------------- display

struct Display {
    frames: Arc<Mutex<tmpsc::Receiver<Frame>>>,
    capturing: Arc<AtomicBool>,
    size: DesktopSize,
}

#[async_trait::async_trait]
impl RdpServerDisplay for Display {
    async fn size(&mut self) -> DesktopSize {
        self.size
    }

    async fn updates(&mut self) -> Result<Box<dyn RdpServerDisplayUpdates>> {
        let rx = self.frames.clone().lock_owned().await;
        self.capturing.store(true, Ordering::Relaxed);
        tracing::info!("client connected");
        Ok(Box::new(Updates {
            rx,
            capturing: self.capturing.clone(),
            prev: None,
            queue: Default::default(),
            size: self.size,
        }))
    }
}

struct Updates {
    rx: OwnedMutexGuard<tmpsc::Receiver<Frame>>,
    capturing: Arc<AtomicBool>,
    /// The last frame we sent, to find what changed.
    prev: Option<Frame>,
    queue: std::collections::VecDeque<DisplayUpdate>,
    size: DesktopSize,
}

impl Drop for Updates {
    fn drop(&mut self) {
        self.capturing.store(false, Ordering::Relaxed);
        tracing::info!("client disconnected");
    }
}

#[async_trait::async_trait]
impl RdpServerDisplayUpdates for Updates {
    async fn next_update(&mut self) -> Result<Option<DisplayUpdate>> {
        loop {
            if let Some(u) = self.queue.pop_front() {
                return Ok(Some(u));
            }
            let Some(frame) = self.rx.recv().await else { return Ok(None) };
            if frame.width as u16 != self.size.width || frame.height as u16 != self.size.height {
                self.size = DesktopSize {
                    width: frame.width as u16,
                    height: frame.height as u16,
                };
                self.queue.push_back(DisplayUpdate::Resize(self.size));
                self.prev = None;
            }
            for rect in changed_rects(self.prev.as_ref(), &frame) {
                if let Some(u) = bitmap(&frame, rect) {
                    self.queue.push_back(DisplayUpdate::Bitmap(u));
                }
            }
            self.prev = Some(frame);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rect {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}

/// Regions that differ from the previous frame, as runs of 64×64 tiles.
fn changed_rects(prev: Option<&Frame>, cur: &Frame) -> Vec<Rect> {
    let (w, h) = (cur.width as usize, cur.height as usize);
    let full = vec![Rect { x: 0, y: 0, w, h }];
    let Some(prev) = prev else { return full };
    if prev.width != cur.width || prev.height != cur.height || prev.stride != cur.stride {
        return full;
    }
    let stride = cur.stride as usize;
    let mut rects = Vec::new();
    let mut changed_tiles = 0usize;
    let tiles_x = w.div_ceil(TILE);
    for ty in (0..h).step_by(TILE) {
        let th = TILE.min(h - ty);
        let mut run: Option<usize> = None;
        for tx in 0..=tiles_x {
            let dirty = tx < tiles_x && {
                let x0 = tx * TILE * 4;
                let x1 = ((tx + 1) * TILE).min(w) * 4;
                (ty..ty + th).any(|row| {
                    let o = row * stride;
                    cur.data[o + x0..o + x1] != prev.data[o + x0..o + x1]
                })
            };
            match (dirty, run) {
                (true, None) => run = Some(tx),
                (false, Some(start)) => {
                    let x = start * TILE;
                    let xe = (tx * TILE).min(w);
                    changed_tiles += tx - start;
                    rects.push(Rect { x, y: ty, w: xe - x, h: th });
                    run = None;
                }
                _ => {}
            }
        }
    }
    // Lots of small updates cost more than one big one.
    let total = tiles_x * h.div_ceil(TILE);
    if changed_tiles * 10 > total * 6 {
        return full;
    }
    rects
}

fn bitmap(frame: &Frame, r: Rect) -> Option<BitmapUpdate> {
    let stride = frame.stride as usize;
    let mut data = Vec::with_capacity(r.w * r.h * 4);
    for row in r.y..r.y + r.h {
        let o = row * stride + r.x * 4;
        data.extend_from_slice(&frame.data[o..o + r.w * 4]);
    }
    Some(BitmapUpdate {
        x: r.x as u16,
        y: r.y as u16,
        width: NonZeroU16::new(r.w as u16)?,
        height: NonZeroU16::new(r.h as u16)?,
        format: if frame.rgb_order { PixelFormat::RgbX32 } else { PixelFormat::BgrX32 },
        data: Bytes::from(data),
        stride: NonZeroUsize::new(r.w * 4)?,
    })
}

// ------------------------------------------------------------------- input

struct InputHandler {
    tx: std::sync::mpsc::Sender<Input>,
}

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;
const BTN_SIDE: u32 = 0x113;
const BTN_EXTRA: u32 = 0x114;

impl RdpServerInputHandler for InputHandler {
    fn keyboard(&mut self, event: KeyboardEvent) {
        tracing::trace!(?event, "keyboard");
        let (code, extended, pressed) = match event {
            KeyboardEvent::Pressed { code, extended } => (code, extended, true),
            KeyboardEvent::Released { code, extended } => (code, extended, false),
            _ => return,
        };
        if let Some(code) = keymap::scancode_to_evdev(code, extended) {
            let _ = self.tx.send(Input::Key { code, pressed });
        }
    }

    fn mouse(&mut self, event: MouseEvent) {
        tracing::trace!(?event, "mouse");
        let ev = match event {
            MouseEvent::Move { x, y } => Input::Motion { x: x.into(), y: y.into() },
            MouseEvent::LeftPressed => Input::Button { code: BTN_LEFT, pressed: true },
            MouseEvent::LeftReleased => Input::Button { code: BTN_LEFT, pressed: false },
            MouseEvent::RightPressed => Input::Button { code: BTN_RIGHT, pressed: true },
            MouseEvent::RightReleased => Input::Button { code: BTN_RIGHT, pressed: false },
            MouseEvent::MiddlePressed => Input::Button { code: BTN_MIDDLE, pressed: true },
            MouseEvent::MiddleReleased => Input::Button { code: BTN_MIDDLE, pressed: false },
            MouseEvent::Button4Pressed => Input::Button { code: BTN_SIDE, pressed: true },
            MouseEvent::Button4Released => Input::Button { code: BTN_SIDE, pressed: false },
            MouseEvent::Button5Pressed => Input::Button { code: BTN_EXTRA, pressed: true },
            MouseEvent::Button5Released => Input::Button { code: BTN_EXTRA, pressed: false },
            // RDP: positive = wheel away from the user (scroll up).
            MouseEvent::VerticalScroll { value } => Input::Scroll {
                vertical: true,
                clicks: -f64::from(value) / 120.0,
            },
            MouseEvent::Scroll { x, y } => {
                if y != 0 {
                    Input::Scroll { vertical: true, clicks: -f64::from(y) / 120.0 }
                } else {
                    Input::Scroll { vertical: false, clicks: f64::from(x) / 120.0 }
                }
            }
            MouseEvent::RelMove { .. } => return,
        };
        let _ = self.tx.send(ev);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(w: u32, h: u32, fill: u8) -> Frame {
        Frame {
            width: w,
            height: h,
            stride: w * 4,
            rgb_order: false,
            data: vec![fill; (w * h * 4) as usize],
        }
    }

    #[test]
    fn first_frame_is_full() {
        let f = frame(100, 50, 0);
        assert_eq!(changed_rects(None, &f), vec![Rect { x: 0, y: 0, w: 100, h: 50 }]);
    }

    #[test]
    fn unchanged_frame_sends_nothing() {
        let a = frame(300, 200, 7);
        let b = frame(300, 200, 7);
        assert!(changed_rects(Some(&a), &b).is_empty());
    }

    #[test]
    fn one_pixel_change_is_one_tile() {
        let a = frame(1920, 1080, 0);
        let mut b = frame(1920, 1080, 0);
        let (x, y) = (700usize, 300usize);
        b.data[(y * 1920 + x) * 4] = 255;
        assert_eq!(changed_rects(Some(&a), &b), vec![Rect { x: 640, y: 256, w: 64, h: 64 }]);
        let u = bitmap(&b, Rect { x: 640, y: 256, w: 64, h: 64 }).unwrap();
        assert_eq!(u.data.len(), 64 * 64 * 4);
    }

    #[test]
    fn edge_tiles_are_clipped() {
        let a = frame(100, 70, 0);
        let b = frame(100, 70, 0);
        let mut b2 = b;
        let last = b2.data.len() - 1;
        b2.data[last] = 1;
        assert_eq!(changed_rects(Some(&a), &b2), vec![Rect { x: 64, y: 64, w: 36, h: 6 }]);
    }
}
