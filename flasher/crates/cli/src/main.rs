//! Thin CLI over the flasher crates. The same engine the GUI drives — this is
//! for power users and scripts.

use ohc_flash_core::{board, image, method, Method};
use ohc_flash_engine::{network, Progress, Release};
use ohc_flash_transport as tp;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("help");
    let rest = &args[args.len().min(1)..];
    let ok = match cmd {
        "boards" => { boards(); true }
        "discover" => discover(),
        "identify" => identify(rest),
        "plan" => plan(rest),
        "validate" => validate(rest),
        "install" => install(rest),
        "rootfs" => rootfs(rest),
        "wrap" => wrap(rest),
        "help" | "-h" | "--help" => { help(); true }
        other => { eprintln!("unknown command '{other}'\n"); help(); false }
    };
    if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

fn help() {
    println!(
        "openHC flasher (CLI)\n\n\
         usage: ohc-flash <command>\n\n\
           discover                 find Control4 units on the network\n\
           identify [HOST]          say what a unit is (auto-discovers if omitted)\n\
             \n\
           Any command taking [HOST] also accepts --password <pw> for a controller\n\
           whose root password is not the factory default.\n\
           boards                   list known boards\n\
           plan <board>            show what installing on <board> would do\n\
           validate <dir|zip>       check a release's images\n\
           install [HOST] --images <dir|zip> [--dry-run] [--yes]\n\
                                    install openHC end to end: writes the kernel and\n\
                                    initramfs, reboots into RAM, then writes p1 and\n\
                                    reboots into it. --no-wait stops after stage 1;\n\
                                    --self-install uses the tiny boot-init instead\n\
           rootfs [HOST] --images <dir|zip> [--yes]\n\
                                    stage 2: write rootfs to p1 (box must be RAM-booted)\n\
           wrap <bzImage> <out> [--header FILE]\n\
                                    wrap a bzImage in a CEFDK container\n\n\
         The GUI (`ohc-flasher`) is the primary front end for non-CLI users.\n"
    );
}

fn boards() {
    for b in board::BOARDS {
        let flags: Vec<&str> = [("switch", b.has_switch), ("wifi", b.has_wifi), ("secure-boot", b.secure_boot)]
            .iter().filter(|(_, on)| *on).map(|(f, _)| *f).collect();
        let flags = if flags.is_empty() { "-".into() } else { flags.join(",") };
        println!("  {:<12} {:<40} {}", b.name, b.desc, flags);
    }
}

fn discover() -> bool {
    println!("scanning...");
    let found = tp::discover(true);
    if found.is_empty() {
        println!("  nothing found. A freshly restored unit may not answer ICMP yet; \
                  try again or pass the IP directly.");
        return false;
    }
    for f in &found {
        println!("  {:<16} {:<18} {}", f.ip, f.mac.clone().unwrap_or_default(), f.via.join(","));
    }
    true
}

fn auto_host() -> Option<String> {
    let found = tp::discover(true);
    match found.len() {
        1 => { println!("  using {}", found[0].ip); Some(found[0].ip.clone()) }
        0 => { println!("  no units found; pass the IP explicitly"); None }
        _ => {
            println!("  multiple units found; pass one explicitly:");
            for f in &found { println!("    {}", f.ip); }
            None
        }
    }
}

fn connect(rest: &[String]) -> Option<(String, tp::Ssh)> {
    let host = rest.iter().find(|a| !a.starts_with("--")).cloned().or_else(auto_host)?;
    // --password handles a calculated or dealer-set root password; the known
    // factory and openHC logins are tried after it.
    let pw = rest.windows(2).find(|w| w[0] == "--password").map(|w| w[1].clone());
    let extra: Vec<String> = pw.into_iter().collect();
    match tp::first_working_login_with(&host, &extra) {
        Some(s) => Some((host, s)),
        None if tp::ssh_port_open(&host) => {
            eprintln!("  {host} is reachable but no password worked — SSH password login may be \
                       disabled or the root password is calculated; pass --password <pw> or use a key");
            None
        }
        None => { eprintln!("  cannot reach {host} over SSH"); None }
    }
}

fn identify(rest: &[String]) -> bool {
    let Some((host, ssh)) = connect(rest) else { return false };
    let id = tp::identify(&ssh);
    println!("  {host}: {}", id.describe());
    for (k, v) in &id.raw { println!("    {k}: {}", v.trim()); }
    id.board.is_some()
}

fn plan(rest: &[String]) -> bool {
    let Some(name) = rest.first() else { eprintln!("usage: ohc-flash plan <board>"); return false };
    let Some(b) = board::by_name(name) else { eprintln!("unknown board '{name}'"); return false };
    let id = ohc_flash_core::Identity { board: Some(b), candidates: vec![b], running: board::Running::Stock, raw: vec![] };
    let (Some(m), rej) = method::choose(&id, None) else { eprintln!("no method for {}", b.name); return false };
    let pl = method::plan(b, m);
    println!("  board:  {} ({})", b.name, b.desc);
    println!("  method: {} — {}", m.name(), m.summary());
    for (rm, why) in rej { println!("  (not {}: {why})", rm.name()); }
    for s in &pl.steps { println!("    - {s}"); }
    for w in &pl.writes { println!("    ! {w}"); }
    println!("  recovery: {}", pl.reversible);
    true
}

fn images_arg(rest: &[String]) -> PathBuf {
    rest.windows(2).find(|w| w[0] == "--images").map(|w| PathBuf::from(&w[1]))
        .unwrap_or_else(|| PathBuf::from("output/images"))
}

fn validate(rest: &[String]) -> bool {
    let path = rest.first().map(PathBuf::from).unwrap_or_else(|| images_arg(rest));
    match Release::open(&path) {
        Ok(rel) => {
            let head = rel.get("bzImage").map(|b| b[..b.len().min(0x400)].to_vec());
            let klen = rel.get("bzImage").map(|b| b.len() as u64).unwrap_or(0);
            let probs = image::ea_problems(head.as_deref(), klen, rel.has("rootfs.ext2"), true);
            if probs.is_empty() {
                println!("  ok: {} — bzImage {klen} B ({} B headroom), rootfs present",
                         rel.source, image::headroom(klen));
                true
            } else {
                for p in probs { println!("  ! {p}"); }
                false
            }
        }
        Err(e) => { eprintln!("  {e}"); false }
    }
}

/// Wrap a bzImage in a CEFDK container.
///
/// This exists so the Buildroot post-image step and the installer agree on the
/// container layout by construction rather than by two implementations staying
/// in sync. board/ea-common/post-image.sh calls it.
fn wrap(rest: &[String]) -> bool {
    let args: Vec<&String> = rest.iter().filter(|a| !a.starts_with("--")).collect();
    if args.len() < 2 {
        eprintln!("usage: ohc-flash wrap <bzImage> <out> [--header FILE]");
        return false;
    }
    let (src, dst) = (Path::new(args[0]), Path::new(args[1]));

    let header = match rest.iter().position(|a| a == "--header") {
        Some(i) => match rest.get(i + 1) {
            Some(h) => match std::fs::read(h) {
                Ok(b) => Some(b),
                Err(e) => { eprintln!("wrap: {h}: {e}"); return false; }
            },
            None => { eprintln!("wrap: --header needs a path"); return false; }
        },
        None => None,
    };

    let kernel = match std::fs::read(src) {
        Ok(b) => b,
        Err(e) => { eprintln!("wrap: {}: {e}", src.display()); return false; }
    };

    // Catch a non-kernel here rather than on hardware: a container built from
    // the wrong file boots to nothing and looks like a driver problem.
    if !image::is_bzimage(&kernel) {
        eprintln!("wrap: {} is not a bzImage (no 0x55aa/HdrS magic)", src.display());
        return false;
    }
    let over = image::headroom(kernel.len() as u64);
    if over < 0 {
        eprintln!("wrap: bzImage is {} B, {} B over CEFDK's bootlinux window — \
                   it would overwrite the loader mid-copy and not boot",
                  kernel.len(), -over);
        return false;
    }

    let out = match image::container(&kernel, header.as_deref()) {
        Ok(v) => v,
        Err(e) => { eprintln!("wrap: {e}"); return false; }
    };
    if let Err(e) = std::fs::write(dst, &out) {
        eprintln!("wrap: {}: {e}", dst.display());
        return false;
    }

    let sw = image::size_word(out.len());
    println!("wrap: bzImage {} bytes, container {} bytes ({} B headroom)",
             kernel.len(), out.len(), over);
    // Not stored in the file: the eMMC boot path reads this big-endian total at
    // raw offset 0x200 of the DEVICE, and the installer writes it there.
    println!("wrap: wrote {}  (eMMC size word for the flasher: 0x{:02x}{:02x}{:02x}{:02x} BE)",
             dst.display(), sw[0], sw[1], sw[2], sw[3]);
    true
}

fn install(rest: &[String]) -> bool {
    let dry = rest.iter().any(|a| a == "--dry-run");
    let yes = rest.iter().any(|a| a == "--yes");
    let Some((host, ssh)) = connect(rest) else { return false };
    let forced = rest.windows(2).find(|w| w[0] == "--board").map(|w| w[1].clone());
    let id = tp::identify(&ssh);
    let board = if let Some(name) = forced {
        match board::by_name(&name) {
            Some(b) => { println!("  target: {} ({}) [forced]", b.name, b.desc); b }
            None => { eprintln!("  unknown board '{name}'"); return false; }
        }
    } else {
        println!("  target: {}", id.describe());
        if !id.certain() {
            eprintln!("  refusing to install without a definite board — pass --board <name>");
            return false;
        }
        id.board.unwrap()
    };

    let (Some(m), rej) = method::choose(&id, None) else { eprintln!("  no method applies"); return false };
    for (rm, why) in &rej { println!("  (not {}: {why})", rm.name()); }
    let pl = method::plan(board, m);
    println!("  method: {} — {}", m.name(), m.summary());
    for s in &pl.steps { println!("    - {s}"); }
    for w in &pl.writes { println!("    ! {w}"); }
    println!("  recovery: {}", pl.reversible);

    if m != Method::Network {
        eprintln!("  only the network method is wired into the CLI so far");
        return false;
    }

    let images = images_arg(rest);
    let rel = match Release::open(&images) { Ok(r) => r, Err(e) => { eprintln!("  {e}"); return false } };
    if dry { println!("\n  (dry run — nothing written)"); return true; }
    if !yes {
        eprint!("\n  proceed with stage 1 (RAM installer)? [y/N] ");
        use std::io::Write; std::io::stdout().flush().ok();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).ok();
        if !matches!(line.trim(), "y" | "yes") { return false; }
    }

    let p = Progress::stdout();
    // The tiny self-installer is opt-in, NOT the default. It writes a separate
    // boot-init whose only job is to lay down p1 on first boot, so a fault in it
    // (a missing dynamic loader, say) leaves a box that loads a kernel and then
    // dies with a garbled console and no network — recoverable only over serial
    // in manufacturing mode. The two-stage flow keeps the full initramfs, which
    // stays reachable over SSH at every step.
    if rest.iter().any(|a| a == "--self-install") {
        if let Err(e) = network::install_self(&ssh, &rel, board.secure_boot, &p) {
            eprintln!("  install failed: {e:#}");
            return false;
        }
        println!("\n  done — the box is rebooting; first boot self-installs p1 and pivots.");
        return true;
    }
    if let Err(e) = network::stage1_ram_installer(&ssh, &rel, board.secure_boot, &p) {
        eprintln!("  stage 1 failed: {e:#}");
        return false;
    }
    println!("\n  stage 1 done — the box is rebooting into the RAM installer.");
    if rest.iter().any(|a| a == "--no-wait") {
        println!("  when it is back (same IP), run:");
        println!("    ohc-flash rootfs {host} --images {}", images.display());
        return true;
    }

    // Finish the job: wait for the RAM-booted openHC and run stage 2 itself.
    // Leaving the box in RAM and telling the user to run a second command is how
    // a half-installed unit gets forgotten about.
    println!("  waiting up to 300s for the box to come back in RAM...");
    let ssh2 = match tp::wait_for_login(&host, 300) {
        Some(s) => { println!("  — up"); s }
        None => {
            eprintln!("  the box did not come back within 300s. When it is up, run:");
            eprintln!("    ohc-flash rootfs {host} --images {}", images.display());
            return false;
        }
    };
    if let Err(e) = network::stage2_write_rootfs(&ssh2, &rel, &p) {
        eprintln!("  stage 2 failed: {e:#}");
        return false;
    }
    println!("\n  done — the box is rebooting into openHC on p1.");
    true
}

fn rootfs(rest: &[String]) -> bool {
    let yes = rest.iter().any(|a| a == "--yes");
    let Some((host, ssh)) = connect(rest) else { return false };
    let images = images_arg(rest);
    let rel = match Release::open(&images) { Ok(r) => r, Err(e) => { eprintln!("  {e}"); return false } };
    println!("  {host}: writing openHC rootfs to p1 (stage 2)");
    if !yes {
        eprint!("  the box must be running from RAM (not p1). proceed? [y/N] ");
        use std::io::Write; std::io::stdout().flush().ok();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).ok();
        if !matches!(line.trim(), "y" | "yes") { return false; }
    }
    let p = Progress::stdout();
    match network::stage2_write_rootfs(&ssh, &rel, &p) {
        Ok(()) => { println!("\n  done — the box is rebooting into openHC on p1."); true }
        Err(e) => { eprintln!("  stage 2 failed: {e:#}"); false }
    }
}
