//! zigbee — bring the EM35x Zigbee NCP up and hold it up.
//!
//! A first-class openHC package (built and staged into the rootfs like iod/webd),
//! gated onto only the boards whose `ohc.features` lists `zigbee`. It replaces the
//! former node script of the same name: the bring-up is a few serial writes, so a
//! ~1 MB static Rust binary beats dragging the Node runtime in just for this.
//! (Node is still pulled by the `zigbee` feature, but for zigbee2mqtt — the
//! MQTT bridge — not for this.)
//!
//! WHY it is needed. The EM35x resets into its serial BOOTLOADER whenever its
//! reset line is asserted, and the bootloader ignores EZSP/ASH. On the CA-1 that
//! reset is the port's DTR, so merely opening the tty (what every EZSP host does)
//! lands in the bootloader and the coordinator never forms. The fix, in order:
//!
//!   1. open the port   (DTR asserted -> on DTR-reset boards the chip drops to
//!                        its bootloader, which echoes)
//!   2. send "2"        (the bootloader's "run application" menu key)
//!   3. ASH RST -> RSTACK confirms the NCP is now running
//!   4. HOLD the port open for the life of the service so DTR stays asserted —
//!      the next opener (zigbee2mqtt) does NOT re-trigger the bootloader.
//!
//! On boards whose reset is a GPIO held released at boot (the EA's gpio29), step 1
//! does not reset anything — the app is already running — and the same sequence
//! simply confirms it. Verified on an EA1: a bare ASH RST returns RSTACK (0xc1).
//!
//! NETWORK RADIO MODE. With `OHC_ZIGBEE_TCP_PORT` set, the held port is also
//! served over TCP — the same thing ser2net does — so a zigpy stack elsewhere
//! (Home Assistant's ZHA: radio type EZSP, path `socket://<box>:<port>`) drives
//! the NCP as if it were plugged in locally. One client at a time; a new
//! connection replaces the old one, so an HA restart is never locked out by a
//! half-open socket from its previous life.
//!
//! This exists because of the HC-800. Its EM357 runs EmberZNet 4.7.2 (EZSP
//! protocol 4), and zigbee2mqtt's drivers need EZSP 13+ (`ember`) — the EM35x
//! cannot run anything that new. zigpy's bellows still speaks EZSP v4. Verified
//! 2026-10-02: `bellows info` over `socket://` read the NCP's version, EUI64 and
//! its existing coordinator network.
//!
//! FIRMWARE. Before holding the port it brings the NCP's firmware up to the image
//! the zigbee feature ships (EmberZNet 6.7.8, the last for the EM35x), the way
//! Control4 reflashes it: see ncp.rs. Once current, a boot only reads the version.
//!
//! Config (board.env): OHC_ZIGBEE_TTY (required; absent => nothing to do),
//! OHC_ZIGBEE_BAUD (the app's baud, tried first; default 115200),
//! OHC_ZIGBEE_TCP_PORT (unset => hold only), OHC_ZIGBEE_TCP_BIND (default
//! 0.0.0.0), OHC_ZIGBEE_NCP_IMAGE (default /usr/share/ohc/zigbee/em357-ncp.ebl),
//! OHC_ZIGBEE_NCP_UPDATE (0 = never flash; default 1), OHC_ZIGBEE_BTL_BAUD (the
//! serial bootloader's baud; default 115200).
//!
//! By hand (stop S91ohczigbee first; nothing else may hold the port):
//!   zigbee --info             find the NCP, print its version
//!   zigbee --flash <img.ebl>  flash an image regardless of version

mod ash;
mod bootloader;
mod ncp;

use std::io;
use std::os::unix::io::RawFd;
use std::thread::sleep;
use std::time::Duration;

fn write_all(fd: RawFd, buf: &[u8]) {
    let mut off = 0;
    while off < buf.len() {
        let n = unsafe {
            libc::write(fd, buf[off..].as_ptr() as *const libc::c_void, buf.len() - off)
        };
        if n > 0 {
            off += n as usize;
        } else {
            break;
        }
    }
}

/// Open a tty raw at `baud`, 8N1, no flow control. Mirrors iod's open_serial;
/// libc termios rather than a serialport crate, which drags a dependency tree
/// (and libudev) in for one tcsetattr and fights musl static cross-builds.
fn open_serial(dev: &str, baud: u32) -> io::Result<RawFd> {
    use std::ffi::CString;
    let path = CString::new(dev).map_err(|_| io::Error::other("bad device path"))?;
    let fd = unsafe {
        libc::open(path.as_ptr(), libc::O_RDWR | libc::O_NOCTTY | libc::O_NONBLOCK)
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    unsafe {
        let mut t: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(fd, &mut t) != 0 {
            let e = io::Error::last_os_error();
            libc::close(fd);
            return Err(e);
        }
        libc::cfmakeraw(&mut t);
        let speed = speed(baud);
        libc::cfsetispeed(&mut t, speed);
        libc::cfsetospeed(&mut t, speed);
        t.c_cflag |= libc::CLOCAL | libc::CREAD;
        t.c_cflag &= !libc::CRTSCTS;
        t.c_cc[libc::VMIN] = 0;
        t.c_cc[libc::VTIME] = 0;
        if libc::tcsetattr(fd, libc::TCSANOW, &t) != 0 {
            let e = io::Error::last_os_error();
            libc::close(fd);
            return Err(e);
        }
        libc::tcflush(fd, libc::TCIOFLUSH);
        // Assert DTR: the CA-1's EM35x reset line. Harmless where reset is a GPIO.
        let bits: libc::c_int = libc::TIOCM_DTR;
        libc::ioctl(fd, libc::TIOCMBIS, &bits);
    }
    Ok(fd)
}

fn speed(baud: u32) -> libc::speed_t {
    match baud {
        9600 => libc::B9600,
        19200 => libc::B19200,
        38400 => libc::B38400,
        57600 => libc::B57600,
        115200 => libc::B115200,
        230400 => libc::B230400,
        _ => libc::B115200,
    }
}

/// Change an open tty's baud in place (the bootloader and an app can differ).
fn set_baud(fd: RawFd, baud: u32) {
    unsafe {
        let mut t: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(fd, &mut t) == 0 {
            libc::cfsetispeed(&mut t, speed(baud));
            libc::cfsetospeed(&mut t, speed(baud));
            libc::tcsetattr(fd, libc::TCSADRAIN, &t);
        }
    }
}

fn env_u32(name: &str) -> Option<u32> {
    std::env::var(name).ok().and_then(|s| s.trim().parse().ok())
}

fn main() {
    let dev = match std::env::var("OHC_ZIGBEE_TTY") {
        Ok(d) if !d.is_empty() => d,
        _ => {
            println!("zigbee: OHC_ZIGBEE_TTY unset — no radio on this board, nothing to do");
            return;
        }
    };
    let baud = env_u32("OHC_ZIGBEE_BAUD").unwrap_or(115200);
    // The app is looked for at its configured baud first, then at the rates
    // EM35x NCP images ship with (a reflash can change it: Telegesis-style
    // images run at 57600).
    let mut app_bauds = vec![baud];
    for b in [115200, 57600] {
        if !app_bauds.contains(&b) {
            app_bauds.push(b);
        }
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let forced = match args.as_slice() {
        [f, img] if f == "--flash" => Some(img.clone()),
        _ => None,
    };
    let image_path = forced
        .clone()
        .or_else(|| std::env::var("OHC_ZIGBEE_NCP_IMAGE").ok().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| ncp::DEFAULT_IMAGE.to_string());
    let image = match ncp::load_image(&image_path) {
        Ok(i) => i,
        Err(e) => {
            println!("zigbee: NCP image unusable ({e}) — not updating");
            None
        }
    };
    if forced.is_some() && image.is_none() {
        eprintln!("zigbee: no image at {image_path}");
        std::process::exit(2);
    }
    let cfg = ncp::Config {
        image,
        update: std::env::var("OHC_ZIGBEE_NCP_UPDATE").map(|v| v.trim() != "0").unwrap_or(true),
        btl_baud: env_u32("OHC_ZIGBEE_BTL_BAUD").unwrap_or(115200),
        app_bauds,
    };

    let fd = match open_serial(&dev, baud) {
        Ok(fd) => fd,
        Err(e) => {
            eprintln!("zigbee: cannot open {dev}: {e}");
            std::process::exit(1);
        }
    };

    if args.first().map(String::as_str) == Some("--info") {
        match ncp::find_app(fd, &cfg) {
            Some((b, v)) => println!("{dev}: EmberZNet {} EZSP v{} stack type {} at {b} baud", ash::stack_str(v.stack), v.protocol, v.stack_type),
            None => println!("{dev}: no NCP app answers{}", if bootloader::at_prompt(fd) { " (the serial bootloader does)" } else { "" }),
        }
        return;
    }
    if let (Some(_), Some(img)) = (&forced, &cfg.image) {
        // By hand: flash regardless of version, from the app if it runs.
        let r = match ncp::find_app(fd, &cfg) {
            Some((b, _)) => {
                set_baud(fd, b);
                let mut a = ash::Ash::new(fd);
                let _ = a.reset() && a.version().is_some();
                ncp::flash(fd, img, cfg.btl_baud, Some(&mut a))
            }
            None => ncp::flash(fd, img, cfg.btl_baud, None),
        };
        match r.map(|_| ncp::find_app(fd, &cfg)) {
            Ok(Some((b, v))) => println!("zigbee: flashed; NCP runs EmberZNet {} at {b} baud", ash::stack_str(v.stack)),
            Ok(None) => println!("zigbee: flashed, but no app answers yet"),
            Err(e) => {
                eprintln!("zigbee: flash failed: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    if ncp::ensure(fd, &cfg).is_none() {
        println!("zigbee: no NCP app answers on {dev} — holding the port anyway");
        set_baud(fd, baud);
    }

    if let Some(port) = std::env::var("OHC_ZIGBEE_TCP_PORT").ok().and_then(|p| p.parse::<u16>().ok()) {
        let bind = std::env::var("OHC_ZIGBEE_TCP_BIND").unwrap_or_else(|_| "0.0.0.0".into());
        serve_tcp(fd, &dev, &bind, port);
    }

    // Hold the fd (and DTR) open forever. Deliberately no reads past bring-up: the
    // radio owner (zigbee2mqtt) reads the EZSP stream, and a tty's input is a
    // single queue — a read here would steal its bytes.
    loop {
        sleep(Duration::from_secs(3600));
    }
}

/// Relay the tty to one TCP client at a time, forever. Never returns unless the
/// listener cannot be created (then the caller falls back to holding the port).
fn serve_tcp(tty: RawFd, dev: &str, bind: &str, port: u16) {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::os::unix::io::AsRawFd;

    let listener = match TcpListener::bind((bind, port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("zigbee: cannot listen on {bind}:{port}: {e} — holding the port only");
            return;
        }
    };
    let _ = listener.set_nonblocking(true);
    println!("zigbee: serving {dev} on tcp {bind}:{port} (zigpy: socket://<this box>:{port})");

    let mut client: Option<TcpStream> = None;
    let mut buf = [0u8; 4096];
    loop {
        let mut fds = vec![
            libc::pollfd { fd: listener.as_raw_fd(), events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: tty, events: libc::POLLIN, revents: 0 },
        ];
        if let Some(c) = &client {
            fds.push(libc::pollfd { fd: c.as_raw_fd(), events: libc::POLLIN, revents: 0 });
        }
        let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, 1000) };
        if n < 0 {
            sleep(Duration::from_millis(50));
            continue;
        }

        // A new connection always wins: the old one is most likely a client
        // that died without closing (an HA restart), not one still in use.
        if fds[0].revents & libc::POLLIN != 0 {
            if let Ok((s, peer)) = listener.accept() {
                let _ = s.set_nodelay(true);
                let _ = s.set_nonblocking(false);
                let _ = s.set_write_timeout(Some(Duration::from_secs(5)));
                if client.is_some() {
                    println!("zigbee: {peer} replaces the previous client");
                } else {
                    println!("zigbee: client {peer}");
                }
                // Stale NCP output belongs to nobody; do not hand it to the
                // new client mid-frame.
                unsafe { libc::tcflush(tty, libc::TCIFLUSH) };
                client = Some(s);
                continue;
            }
        }

        // tty -> client. Read even with no client, so the queue cannot back up
        // into the NCP; with no client the bytes have no owner and are dropped.
        if fds[1].revents & libc::POLLIN != 0 {
            let r = unsafe { libc::read(tty, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
            if r > 0 {
                if let Some(c) = &mut client {
                    if c.write_all(&buf[..r as usize]).is_err() {
                        println!("zigbee: client gone (write failed)");
                        client = None;
                    }
                }
            }
        }

        // client -> tty
        if fds.len() > 2 && fds[2].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
            let gone = match client.as_mut().map(|c| c.read(&mut buf)) {
                Some(Ok(0)) | Some(Err(_)) | None => true,
                Some(Ok(r)) => {
                    write_all(tty, &buf[..r]);
                    false
                }
            };
            if gone {
                println!("zigbee: client disconnected");
                client = None;
            }
        }
    }
}
