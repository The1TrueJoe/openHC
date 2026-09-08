//! Host UART bridge — ONE port, MANY viewers.
//!
//! The obvious design opens the tty per WebSocket. That breaks the moment two
//! people have the config GUI open: both get half the bytes, and which half is a
//! race. It is the same reason iod exists at all, reproduced inside iod.
//!
//! So a port has ONE session. The first client opens it, the last one to leave
//! closes it, and in between every client sees the same stream and any client
//! can type into it. Two installers on the same console see the same session —
//! including each other's keystrokes, which is what makes it usable as a shared
//! console rather than two people fighting over a cable.
//!
//! New arrivals get the recent scrollback replayed, so joining a session mid-way
//! does not mean staring at a blank screen until the far end says something.
use crate::events::Bus;
use crate::mcu::open_serial;
use std::collections::{HashMap, VecDeque};
use std::io;
use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::unix::AsyncFd;
use serde_json::json;
use tokio::sync::{broadcast, mpsc};

/// How much history a joining client is given. 32 KB is a couple of screens of
/// a boot log — enough to see what just happened, small enough that a chatty
/// port cannot grow this without bound.
const SCROLLBACK: usize = 32 * 1024;

pub struct Session {
    pub index: usize,
    pub dev: String,
    baud: AtomicU32,
    /// Bytes from the port, fanned out to every client.
    rx: broadcast::Sender<Vec<u8>>,
    /// Bytes from any client, to the port.
    tx: mpsc::UnboundedSender<Vec<u8>>,
    /// Asks the reader task to reopen at a new baud.
    reopen: mpsc::UnboundedSender<u32>,
    scrollback: Mutex<VecDeque<u8>>,
    clients: AtomicUsize,
}

impl Session {
    pub fn baud(&self) -> u32 {
        self.baud.load(Ordering::Relaxed)
    }
    pub fn subscribe(&self) -> broadcast::Receiver<Vec<u8>> {
        self.rx.subscribe()
    }
    pub fn write(&self, b: Vec<u8>) {
        let _ = self.tx.send(b);
    }
    pub fn set_baud(&self, baud: u32) {
        self.baud.store(baud, Ordering::Relaxed);
        let _ = self.reopen.send(baud);
    }
    pub fn history(&self) -> Vec<u8> {
        self.scrollback.lock().map(|s| s.iter().copied().collect()).unwrap_or_default()
    }
    /// Viewer count is STATE — it is how the UI tells one installer that
    /// somebody else is on the same console, which is the whole point of
    /// sharing the session rather than fighting over the cable.
    pub fn join(&self, bus: &Bus) -> usize {
        let n = self.clients.fetch_add(1, Ordering::Relaxed) + 1;
        bus.set(&format!("serial/{}/viewers", self.index), json!(n));
        n
    }
    pub fn leave(&self, bus: &Bus) -> usize {
        let n = self.clients.fetch_sub(1, Ordering::Relaxed).saturating_sub(1);
        bus.set(&format!("serial/{}/viewers", self.index), json!(n));
        n
    }
    pub fn viewers(&self) -> usize {
        self.clients.load(Ordering::Relaxed)
    }
}

#[derive(Default)]
pub struct Hub {
    ports: Mutex<HashMap<usize, Arc<Session>>>,
}

impl Hub {
    /// Get the session for a port, starting it if this is the first caller.
    pub fn session(&self, index: usize, dev: &str, baud: u32, bus: &Bus) -> io::Result<Arc<Session>> {
        let mut ports = self.ports.lock().unwrap();
        if let Some(s) = ports.get(&index) {
            return Ok(s.clone());
        }
        let (rx_tx, _) = broadcast::channel::<Vec<u8>>(256);
        let (tx_tx, tx_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let (re_tx, re_rx) = mpsc::unbounded_channel::<u32>();
        let sess = Arc::new(Session {
            index,
            dev: dev.to_string(),
            baud: AtomicU32::new(baud),
            rx: rx_tx.clone(),
            tx: tx_tx,
            reopen: re_tx,
            scrollback: Mutex::new(VecDeque::with_capacity(SCROLLBACK)),
            clients: AtomicUsize::new(0),
        });
        // Prove the port is openable before handing back a session that looks
        // live but never produces a byte.
        let fd = open_serial(dev, baud)?;
        bus.set(&format!("serial/{index}/baud"), json!(baud));
        bus.set(&format!("serial/{index}/viewers"), json!(0));
        // `tokio::spawn`, NOT spawn_local: this is reached from a request
        // handler, and axum spawns each connection with `tokio::spawn`, so
        // there is no LocalSet in scope here to spawn onto. The runtime is
        // current-thread either way, so the pump still runs on this thread.
        tokio::spawn(pump(sess.clone(), fd, tx_rx, re_rx, bus.clone()));
        ports.insert(index, sess.clone());
        Ok(sess)
    }
}

/// The one task that owns the fd.
async fn pump(
    sess: Arc<Session>,
    first_fd: std::os::unix::io::RawFd,
    mut tx_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    mut reopen: mpsc::UnboundedReceiver<u32>,
    bus: Bus,
) {
    let mut afd = match AsyncFd::new(unsafe { OwnedFd::from_raw_fd(first_fd) }) {
        Ok(a) => a,
        Err(_) => return,
    };
    let mut buf = [0u8; 2048];
    loop {
        tokio::select! {
            // Reopen at a new baud. Done by closing and reopening rather than
            // tcsetattr on the live fd so a half-applied termios cannot leave
            // the port in a state nobody asked for.
            Some(b) = reopen.recv() => {
                drop(afd);
                match open_serial(&sess.dev, b).and_then(|f| AsyncFd::new(unsafe { OwnedFd::from_raw_fd(f) }).map_err(Into::into)) {
                    Ok(a) => {
                        afd = a;
                        let note = format!("\r\n[iod] {} reopened at {} baud\r\n", sess.dev, b);
                        let _ = sess.rx.send(note.into_bytes());
                    }
                    // The port is gone. Say so on the stream the humans are
                    // watching, then stop — a silent dead session is worse.
                    Err(e) => {
                        let _ = sess.rx.send(format!("\r\n[iod] cannot reopen {}: {}\r\n", sess.dev, e).into_bytes());
                        return;
                    }
                }
            }
            Some(w) = tx_rx.recv() => {
                let mut b = &w[..];
                while !b.is_empty() {
                    let Ok(mut g) = afd.writable().await else { return };
                    let n = unsafe { libc::write(g.get_inner().as_raw_fd(), b.as_ptr() as *const _, b.len()) };
                    if n > 0 { b = &b[n as usize..]; } else { g.clear_ready(); }
                }
            }
            r = afd.readable() => {
                let Ok(mut g) = r else { return };
                let n = unsafe { libc::read(g.get_inner().as_raw_fd(), buf.as_mut_ptr() as *mut _, buf.len()) };
                if n > 0 {
                    let chunk = buf[..n as usize].to_vec();
                    if let Ok(mut sb) = sess.scrollback.lock() {
                        for &c in &chunk { sb.push_back(c); }
                        while sb.len() > SCROLLBACK { sb.pop_front(); }
                    }
                    // Err only means no terminal is attached right now; the
                    // port stays open and the scrollback keeps filling.
                    let _ = sess.rx.send(chunk.clone());
                    // Received serial is an EVENT, not state: it is a thing
                    // that happened, it has no value between occurrences, and
                    // an external system may want to trigger on it without
                    // holding a terminal open at all.
                    bus.event(&format!("serial/{}/rx", sess.index),
                              json!({ "b64": crate::b64::encode(&chunk) }));
                } else {
                    g.clear_ready();
                }
            }
        }
    }
}
