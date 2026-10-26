//! Keep the EM35x NCP on the firmware openHC ships, the way Control4 does: at
//! boot, read the running EmberZNet version over EZSP and, when it is older than
//! the bundled image, send the radio to its serial bootloader and upload the
//! image. Once the radio runs that version every later boot just reads it and
//! moves on, so the flash happens once per radio.
//!
//! Every Zigbee board (CA-1, the EA family, the HC-800) carries an EM357 whose
//! only UART-capable serial controller is SC1 on PB1/PB2, so one UART NCP image
//! fits them all; what can differ is the baud rate an image runs at, which is
//! why the app is looked for at more than one.
//!
//! Failure is never fatal to the radio. An NCP with no standalone bootloader
//! refuses launchStandaloneBootloader and keeps running what it has; an upload
//! that dies part-way leaves the bootloader resident with no valid app, and the
//! next boot finds it there and flashes again.

use crate::ash::{self, Ash, NcpVersion};
use crate::{bootloader, set_baud, write_all};
use std::os::unix::io::RawFd;
use std::time::Duration;

/// Where the zigbee feature installs the image, and its version alongside.
pub const DEFAULT_IMAGE: &str = "/usr/share/ohc/zigbee/em357-ncp.ebl";
const STATUS: &str = "/run/ohc-zigbee.ncp";

pub struct Image {
    pub path: String,
    pub data: Vec<u8>,
    pub stack: u16,
}

/// The image and its EmberZNet version (`<image>.version`, e.g. "6.7.8.0").
/// Ok(None) when there is no image on this board.
pub fn load_image(path: &str) -> Result<Option<Image>, String> {
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{path}: {e}")),
    };
    let vfile = format!("{path}.version");
    let v = std::fs::read_to_string(&vfile).map_err(|e| format!("{vfile}: {e}"))?;
    let stack = ash::parse_stack(&v).ok_or_else(|| format!("{vfile}: not a version: {:?}", v.trim()))?;
    Ok(Some(Image { path: path.to_string(), data, stack }))
}

pub struct Config {
    pub image: Option<Image>,
    pub update: bool,
    pub btl_baud: u32,
    pub app_bauds: Vec<u32>,
}

/// Run the app (a "2" to a bootloader that is waiting; ignored by a running
/// app) and find it: ASH reset + EZSP version at each candidate baud. On
/// success the tty is left at the baud the app answered on.
pub fn find_app(fd: RawFd, cfg: &Config) -> Option<(u32, NcpVersion)> {
    set_baud(fd, cfg.btl_baud);
    write_all(fd, b"\r\n");
    let _ = bootloader::read_until(fd, 1200, b"");
    write_all(fd, b"2");
    let _ = bootloader::read_until(fd, 2000, b"");
    for _round in 0..2 {
        for &b in &cfg.app_bauds {
            set_baud(fd, b);
            unsafe { libc::tcflush(fd, libc::TCIOFLUSH) };
            let mut a = Ash::new(fd);
            if a.reset() {
                if let Some(v) = a.version() {
                    return Some((b, v));
                }
            }
        }
    }
    None
}

/// Upload `img`. `running` is the live session when the app is up (it is asked
/// to launch its bootloader first); None when the bootloader is already there.
pub fn flash(fd: RawFd, img: &Image, btl_baud: u32, running: Option<&mut Ash>) -> Result<(), String> {
    if let Some(a) = running {
        match a.launch_bootloader() {
            Some(0) => {}
            Some(s) => {
                return Err(format!(
                    "the NCP refused to launch its bootloader (EmberStatus 0x{s:02x}): no standalone serial bootloader on this radio"
                ))
            }
            None => return Err("no answer to launchStandaloneBootloader".into()),
        }
        std::thread::sleep(Duration::from_millis(1500));
    }
    set_baud(fd, btl_baud);
    if !bootloader::at_prompt(fd) {
        return Err(format!("no bootloader prompt at {btl_baud} baud"));
    }
    if let Some(b) = bootloader::banner(fd) {
        println!("ohc-zigbee: bootloader: {b}");
    }
    println!("ohc-zigbee: uploading {} ({} bytes)", img.path, img.data.len());
    let mut last = 0;
    bootloader::upload(fd, &img.data, |done, total| {
        let pct = done * 100 / total;
        if pct >= last + 25 || done == total {
            println!("ohc-zigbee: … {pct}%");
            last = pct;
        }
    })?;
    let _ = bootloader::read_until(fd, 3000, bootloader::PROMPT);
    bootloader::run_app(fd);
    std::thread::sleep(Duration::from_millis(2000));
    Ok(())
}

fn status(baud: u32, v: &NcpVersion) {
    let _ = std::fs::write(
        STATUS,
        format!("stack={}\nprotocol={}\nbaud={baud}\n", ash::stack_str(v.stack), v.protocol),
    );
}

fn describe(v: &NcpVersion) -> String {
    format!("EmberZNet {} (EZSP v{})", ash::stack_str(v.stack), v.protocol)
}

/// The boot path: find the NCP, bring its firmware up to the image's version if
/// it is behind, and leave the tty at the baud the app talks. Returns that baud
/// and version, or None when no app answers (the caller holds the port anyway).
pub fn ensure(fd: RawFd, cfg: &Config) -> Option<(u32, NcpVersion)> {
    let found = find_app(fd, cfg);
    let img = cfg.image.as_ref().filter(|_| cfg.update);

    let Some((baud, v)) = found else {
        // No app answered. A resident bootloader means an earlier upload never
        // finished (or the app is gone): finish the job.
        set_baud(fd, cfg.btl_baud);
        if !bootloader::at_prompt(fd) {
            return None;
        }
        let Some(img) = img else {
            println!("ohc-zigbee: the NCP is in its bootloader with no valid app, and there is no image to flash");
            return None;
        };
        println!("ohc-zigbee: the NCP is in its bootloader with no valid app — flashing {}", ash::stack_str(img.stack));
        if let Err(e) = flash(fd, img, cfg.btl_baud, None) {
            println!("ohc-zigbee: flash failed: {e} — the bootloader stays resident; the next start retries");
            return None;
        }
        let after = find_app(fd, cfg);
        report_after(&after, img);
        return after;
    };

    println!("ohc-zigbee: NCP runs {} at {baud} baud", describe(&v));
    match img {
        Some(img) if v.stack < img.stack => {
            println!(
                "ohc-zigbee: updating the NCP {} -> {} from {}",
                ash::stack_str(v.stack),
                ash::stack_str(img.stack),
                img.path
            );
            set_baud(fd, baud);
            let mut a = Ash::new(fd);
            if !(a.reset() && a.version().is_some()) {
                println!("ohc-zigbee: lost the NCP before the update; keeping {}", ash::stack_str(v.stack));
                status(baud, &v);
                return Some((baud, v));
            }
            if let Err(e) = flash(fd, img, cfg.btl_baud, Some(&mut a)) {
                println!("ohc-zigbee: update failed: {e}");
            }
            let after = find_app(fd, cfg);
            report_after(&after, img);
            after
        }
        Some(img) if v.stack > img.stack => {
            println!("ohc-zigbee: NCP is newer than the bundled {} — left as it is", ash::stack_str(img.stack));
            status(baud, &v);
            Some((baud, v))
        }
        _ => {
            status(baud, &v);
            Some((baud, v))
        }
    }
}

fn report_after(after: &Option<(u32, NcpVersion)>, img: &Image) {
    match after {
        Some((b, v)) if v.stack == img.stack => {
            println!("ohc-zigbee: NCP updated: {} at {b} baud", describe(v));
            status(*b, v);
        }
        Some((b, v)) => {
            println!(
                "ohc-zigbee: NCP answers {} at {b} baud, not the {} it was given",
                describe(v),
                ash::stack_str(img.stack)
            );
            status(*b, v);
        }
        None => println!("ohc-zigbee: no app answers after the flash — the next start retries"),
    }
}

#[cfg(test)]
mod tests {
    //! An end-to-end run against a simulated EM357 on a socket pair: an
    //! EmberZNet 4.7.2 app (EZSP v4) that, told to launch its bootloader, shows
    //! the real menu, takes an XMODEM-CRC upload (checking every block), and on
    //! "2" comes back as the uploaded 6.7.8 app speaking EZSP v8.
    use super::*;
    use crate::ash::{crc16, frame, randomize, Deframer};
    use std::io::{Read, Write};
    use std::os::unix::io::AsRawFd;
    use std::os::unix::net::UnixStream;
    use std::sync::{Arc, Mutex};

    enum Mode {
        App { protocol: u8, stack: u16 },
        Bootloader,
        Upload,
    }

    fn fake_ncp(mut s: UnixStream, received: Arc<Mutex<Vec<u8>>>, stuck_in_bootloader: bool) {
        let _ = s.set_read_timeout(Some(Duration::from_millis(50)));
        let mut mode = if stuck_in_bootloader { Mode::Bootloader } else { Mode::App { protocol: 4, stack: 0x4720 } };
        let (mut de, mut tx, mut buf, mut xm) = (Deframer::default(), 0u8, [0u8; 512], Vec::<u8>::new());
        let menu = b"\r\nEM357 Serial Btl v5.4.1\r\n1. upload ebl\r\n2. run\r\n3. ebl info\r\nBL > ";
        let mut deadline = std::time::Instant::now() + Duration::from_secs(60);
        while std::time::Instant::now() < deadline {
            let n = match s.read(&mut buf) {
                Ok(0) => return,
                Ok(n) => n,
                Err(_) => continue,
            };
            deadline = std::time::Instant::now() + Duration::from_secs(60);
            let bytes = buf[..n].to_vec();
            match &mut mode {
                Mode::App { protocol, stack } => {
                    for f in de.push(&bytes) {
                        if f[0] == 0xc0 {
                            tx = 0;
                            s.write_all(&frame(0xc1, &[0x02, 0x0b])).unwrap();
                            continue;
                        }
                        if f[0] & 0x80 != 0 {
                            continue; // the host's ACKs
                        }
                        let ack = ((f[0] >> 4) + 1) & 7;
                        let d = randomize(&f[1..]);
                        let extended = d.len() >= 5 && d[2] == 0x01 && *protocol >= 8;
                        let (seq, id, params) = if extended {
                            (d[0], u16::from_le_bytes([d[3], d[4]]), &d[5..])
                        } else {
                            (d[0], d[2] as u16, &d[3..])
                        };
                        let resp: Vec<u8> = match id {
                            0x0000 => vec![*protocol, 2, *stack as u8, (*stack >> 8) as u8],
                            0x008f => vec![0x00],
                            _ => vec![],
                        };
                        let _ = params;
                        let mut p = if extended { vec![seq, 0x80, 0x01, id as u8, (id >> 8) as u8] } else { vec![seq, 0x80, id as u8] };
                        p.extend(resp);
                        s.write_all(&frame((tx << 4) | ack, &randomize(&p))).unwrap();
                        tx = (tx + 1) & 7;
                        if id == 0x008f {
                            mode = Mode::Bootloader;
                            break;
                        }
                    }
                }
                Mode::Bootloader => {
                    if bytes.contains(&b'\n') {
                        s.write_all(menu).unwrap();
                    } else if bytes.contains(&b'1') {
                        received.lock().unwrap().clear();
                        xm.clear();
                        s.write_all(b"\r\nbegin upload\r\nC").unwrap();
                        mode = Mode::Upload;
                    } else if bytes.contains(&b'2') && !received.lock().unwrap().is_empty() {
                        mode = Mode::App { protocol: 8, stack: 0x6780 };
                        de = Deframer::default();
                    }
                }
                Mode::Upload => {
                    xm.extend_from_slice(&bytes);
                    loop {
                        if xm.first() == Some(&0x04) {
                            s.write_all(&[0x06]).unwrap();
                            s.write_all(b"\r\nSerial upload complete\r\n").unwrap();
                            s.write_all(menu).unwrap();
                            mode = Mode::Bootloader;
                            break;
                        }
                        if xm.len() < 133 {
                            break;
                        }
                        let pkt: Vec<u8> = xm.drain(..133).collect();
                        let ok = pkt[0] == 0x01 && pkt[1] == !pkt[2]
                            && crc16(&pkt[3..131], 0) == u16::from_be_bytes([pkt[131], pkt[132]]);
                        if ok {
                            received.lock().unwrap().extend_from_slice(&pkt[3..131]);
                        }
                        s.write_all(&[if ok { 0x06 } else { 0x15 }]).unwrap();
                    }
                }
            }
        }
    }

    #[test]
    fn updates_an_old_ncp_end_to_end() {
        let (host, ncp) = UnixStream::pair().unwrap();
        host.set_nonblocking(true).unwrap();
        let received = Arc::new(Mutex::new(Vec::new()));
        let r2 = received.clone();
        let t = std::thread::spawn(move || fake_ncp(ncp, r2, false));

        let data: Vec<u8> = (0..1000u32).map(|i| (i * 7 % 251) as u8).collect();
        let cfg = Config {
            image: Some(Image { path: "test.ebl".into(), data: data.clone(), stack: 0x6780 }),
            update: true,
            btl_baud: 115200,
            app_bauds: vec![115200],
        };
        let got = ensure(host.as_raw_fd(), &cfg);
        let (_, v) = got.expect("the updated NCP should answer");
        assert_eq!((v.protocol, v.stack), (8, 0x6780));
        let rx = received.lock().unwrap().clone();
        assert_eq!(&rx[..data.len()], &data[..], "image bytes must arrive intact");
        assert!(rx[data.len()..].iter().all(|&b| b == 0x1a), "padding is SUB");

        // A second boot finds it current and does not flash again.
        received.lock().unwrap().clear();
        let again = ensure(host.as_raw_fd(), &cfg).expect("still answers");
        assert_eq!(again.1.stack, 0x6780);
        assert!(received.lock().unwrap().is_empty(), "no second flash");
        drop(host);
        let _ = t.join();
    }

    #[test]
    fn finishes_an_interrupted_flash() {
        // The radio is in its bootloader with no valid app (an upload died).
        let (host, ncp) = UnixStream::pair().unwrap();
        host.set_nonblocking(true).unwrap();
        let received = Arc::new(Mutex::new(Vec::new()));
        let r2 = received.clone();
        let t = std::thread::spawn(move || fake_ncp(ncp, r2, true));
        let cfg = Config {
            image: Some(Image { path: "test.ebl".into(), data: vec![0x5a; 300], stack: 0x6780 }),
            update: true,
            btl_baud: 115200,
            app_bauds: vec![115200],
        };
        let (_, v) = ensure(host.as_raw_fd(), &cfg).expect("flashed from the bootloader");
        assert_eq!(v.stack, 0x6780);
        assert_eq!(&received.lock().unwrap()[..300], &[0x5a; 300][..]);
        drop(host);
        let _ = t.join();
    }
}
