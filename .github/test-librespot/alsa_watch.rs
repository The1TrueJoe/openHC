// TEST ONLY (openHC): exercise AlsaMixer::volume_changes against a real ALSA
// control (snd-dummy on the CI runner, or a box's own card).
//   alsa_watch <device> <control>
use librespot::playback::mixer::{Mixer, MixerConfig, alsamixer::AlsaMixer};
use std::process::Command;
use std::time::Duration;
use tokio::time::timeout;

fn amixer(device: &str, control: &str, value: &str) {
    let ok = Command::new("amixer")
        .args(["-q", "-D", device, "sset", control, value])
        .status()
        .expect("amixer")
        .success();
    assert!(ok, "amixer sset {control} {value} failed");
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut args = std::env::args().skip(1);
    let device = args.next().unwrap_or_else(|| "hw:Dummy".into());
    let control = args.next().unwrap_or_else(|| "Master".into());
    let mixer = AlsaMixer::open(MixerConfig {
        device: device.clone(),
        control: control.clone(),
        ..Default::default()
    })
    .expect("open mixer");
    let mut changes = mixer.volume_changes().expect("mixer is watchable");
    let quiet = Duration::from_secs(2);

    // 1. librespot's own writes are not reported, not even a burst of them.
    for v in (0..=65535u32).step_by(1311) {
        mixer.set_volume(v as u16);
    }
    mixer.set_volume(20000);
    assert!(
        timeout(quiet, changes.changed()).await.is_err(),
        "an own write was reported as an outside change: {}",
        *changes.borrow()
    );
    println!("1 ok: own writes not reported (mixer reads {})", mixer.volume());

    // 2. An outside write is reported, with the volume the mixer reads.
    amixer(&device, &control, "75%");
    timeout(quiet, changes.changed())
        .await
        .expect("outside change not reported in time")
        .expect("watcher gone");
    let reported = *changes.borrow_and_update();
    assert_eq!(reported, mixer.volume());
    println!("2 ok: outside change reported as {reported}");

    // 3. Back to a level librespot set earlier: still an outside change.
    amixer(&device, &control, "20000");
    timeout(quiet, changes.changed())
        .await
        .expect("second outside change not reported")
        .expect("watcher gone");
    println!("3 ok: reported {}", *changes.borrow_and_update());

    // 4. Own write after outside changes: quiet again.
    mixer.set_volume(40000);
    assert!(timeout(quiet, changes.changed()).await.is_err(), "own write reported after outside change");
    println!("4 ok");

    // 5. Mute through the switch, if the control has one.
    if Command::new("amixer").args(["-q", "-D", &device, "sset", &control, "mute"]).status().map(|s| s.success()).unwrap_or(false) {
        match timeout(quiet, changes.changed()).await {
            Ok(_) => println!("5 ok: mute reported as {}", *changes.borrow_and_update()),
            Err(_) => println!("5: no switch on this control (nothing reported)"),
        }
    }
    // 6. A second subscriber (Spirc after a reconnect) does not replay a change
    //    the first one already saw.
    amixer(&device, &control, "55%");
    timeout(quiet, changes.changed()).await.expect("55% not reported").expect("watcher gone");
    changes.borrow_and_update();
    let mut later = mixer.volume_changes().expect("second subscriber");
    assert!(timeout(quiet, later.changed()).await.is_err(), "a new subscriber replayed an old change");
    amixer(&device, &control, "45%");
    timeout(quiet, later.changed()).await.expect("45% not reported to the new subscriber").expect("watcher gone");
    println!("6 ok: new subscriber sees only new changes ({})", *later.borrow_and_update());

    // 7. The watcher thread stops once the mixer and every receiver are gone.
    let watching = || {
        std::fs::read_dir("/proc/self/task").unwrap().filter(|t| {
            let comm = t.as_ref().unwrap().path().join("comm");
            std::fs::read_to_string(comm).map(|c| c.trim() == "alsa-mixer-watch").unwrap_or(false)
        }).count()
    };
    assert_eq!(watching(), 1, "expected exactly one watcher thread");
    drop(changes);
    drop(later);
    drop(mixer);
    std::thread::sleep(Duration::from_millis(2500));
    assert_eq!(watching(), 0, "watcher thread still running after everything was dropped");
    println!("7 ok: watcher thread stopped");
    println!("ALL OK");
}
