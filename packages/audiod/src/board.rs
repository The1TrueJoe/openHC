//! What the board has: its ALSA PCMs, as the kernel registered them.
//!
//! Every playback PCM is an output and every capture PCM an input
//! (`/proc/asound/card*/pcm*{p,c}/info`). The PCM's name is its label: a
//! device-tree sound card names each link, the EA's HDMI card calls its PCM
//! "HDMI". A codec that cannot name its PCMs (the HC-800's HDA) gets them from
//! the board's udev rule, as properties on `/dev/snd/pcmC<n>D<d><p|c>`:
//!
//!   OHC_AUDIO_LABEL   the label; where any of a card's playback (or capture)
//!                     PCMs has one, its others in that direction are not used
//!   OHC_AUDIO_SWAP=1  left and right are exchanged between PCM and jack
//!
//! A port's DSP profile comes from its codec's `firmware-name` in the device
//! tree: `openhc/adau1451-ea3-openhc.bin` -> [`DSP_DIR`]`/adau1451-ea3-openhc.json`.
//! The id (`analog1`, `hdmi`) is the label in lower case without spaces or
//! anything in parentheses; saved endpoints refer to it. A port's sample
//! format is S16_LE where its hardware takes it, else S32_LE (the EA's I2S,
//! S/PDIF and HDMI), as `aplay --dump-hw-params` reports it.
//!
//! board.env adds software defaults only: OHC_AUDIO_RATE (the dmix rate) and
//! OHC_AUDIO_HELPERS. A board with no PCM has no endpoint map; the daemon then
//! only reports the receivers and handles output/volume.
use crate::tone::Knob;
use serde::Serialize;
use std::path::{Path, PathBuf};

pub const ASOUND: &str = "/proc/asound";
pub const DSP_DIR: &str = "/usr/share/openhc/dsp";

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct Port {
    pub id: String,
    pub label: String,
    /// The ALSA hardware device behind it.
    pub device: String,
    /// The shared PCM endpoints play into (dmix for outputs, dsnoop for inputs).
    pub pcm: String,
    /// Left and right exchanged between the PCM and the jack.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub swap: bool,
    /// The DSP profile behind this port's tone.
    #[serde(skip)]
    pub dsp: Option<String>,
    /// The hardware sample format its dmix/dsnoop and tone stage run at; None is S16_LE.
    #[serde(skip)]
    pub format: Option<String>,
    /// The tone knobs this port has, in the order the UI shows them; empty
    /// when it has no tone stage. Filled in once the board is read (tone.rs).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tone: Vec<Knob>,
}

#[derive(Clone, Debug, Default)]
pub struct Board {
    pub outputs: Vec<Port>,
    pub inputs: Vec<Port>,
    pub rate: u32,
    pub helpers: Vec<String>,
    /// The product, for default endpoint names: `EA-3`, `HC-800`.
    pub product: String,
}

fn env(k: &str) -> String {
    std::env::var(k).unwrap_or_default()
}

/// One PCM stream the kernel registered.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pcm {
    pub card: u32,
    /// The card's id: `HDMI`, `Intel`, `openhcea`.
    pub card_id: String,
    pub device: u32,
    pub capture: bool,
    /// The PCM's own name: `name:` in its info, else `id:`.
    pub name: String,
    /// From the board's udev rule.
    pub label: Option<String>,
    pub swap: bool,
    pub dsp: Option<String>,
}

/// `key: value` lines, as /proc/asound writes them.
fn field<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix(':')).map(str::trim)
}

/// Every PCM stream under `root` (/proc/asound).
pub fn pcms(root: &Path) -> Vec<Pcm> {
    let mut v = vec![];
    let Ok(cards) = std::fs::read_dir(root) else { return v };
    for c in cards.flatten() {
        let name = c.file_name().to_string_lossy().into_owned();
        let Some(card) = name.strip_prefix("card").and_then(|n| n.parse().ok()) else { continue };
        let card_id = std::fs::read_to_string(c.path().join("id")).unwrap_or_default().trim().to_string();
        let Ok(streams) = std::fs::read_dir(c.path()) else { continue };
        for s in streams.flatten() {
            let Ok(info) = std::fs::read_to_string(s.path().join("info")) else { continue };
            let sn = s.file_name().to_string_lossy().into_owned();
            let Some(rest) = sn.strip_prefix("pcm") else { continue };
            let capture = rest.ends_with('c');
            let Ok(device) = rest.trim_end_matches(['p', 'c']).parse() else { continue };
            let pick = |k| field(&info, k).filter(|s| !s.is_empty());
            let name = pick("name").or_else(|| pick("id")).unwrap_or_default().to_string();
            v.push(Pcm { card, card_id: card_id.clone(), device, capture, name, ..Pcm::default() });
        }
    }
    v.sort_by_key(|p| (p.card, p.device, p.capture));
    v
}

/// The label a PCM's name gives. ASoC names a link's PCM
/// `<link> <codec dai>-<device>`; the link is the label.
pub fn label_of(name: &str, device: u32) -> String {
    match name.rsplit_once(' ') {
        Some((link, dai)) if dai.strip_suffix(&format!("-{device}")).is_some_and(|d| !d.is_empty()) => link.to_string(),
        _ => name.to_string(),
    }
}

/// `Analog 1` -> `analog1`, `Coax (S/PDIF)` -> `coax`.
pub fn slug(label: &str) -> String {
    let mut s = String::new();
    let mut depth = 0;
    for ch in label.chars() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            c if depth == 0 && c.is_ascii_alphanumeric() => s.push(c.to_ascii_lowercase()),
            _ => {}
        }
    }
    if s.is_empty() { "out".into() } else { s }
}

/// Outputs or inputs from the PCMs, HDMI last, then by label.
pub fn ports(pcms: &[Pcm], capture: bool, pcm_prefix: &str) -> Vec<Port> {
    let labelled = |card: u32| pcms.iter().any(|p| p.card == card && p.capture == capture && p.label.is_some());
    let mut v: Vec<Port> = pcms
        .iter()
        .filter(|p| p.capture == capture && (p.label.is_some() || !labelled(p.card)))
        .map(|p| Port {
            label: p.label.clone().unwrap_or_else(|| label_of(&p.name, p.device)),
            device: format!("hw:CARD={},DEV={}", p.card_id, p.device),
            swap: p.swap,
            dsp: p.dsp.clone(),
            ..Port::default()
        })
        .filter(|p| !p.label.is_empty())
        .collect();
    v.sort_by_key(|p| (p.label.contains("HDMI"), p.label.clone()));
    let mut seen = std::collections::HashSet::new();
    for p in &mut v {
        let base = slug(&p.label);
        let mut id = base.clone();
        let mut n = 2;
        while !seen.insert(id.clone()) {
            id = format!("{base}{n}");
            n += 1;
        }
        p.pcm = format!("{pcm_prefix}{id}");
        p.id = id;
    }
    v
}

/// The board's udev properties for one PCM node.
fn udev(p: &Pcm) -> String {
    let node = format!("/dev/snd/pcmC{}D{}{}", p.card, p.device, if p.capture { 'c' } else { 'p' });
    std::process::Command::new("udevadm")
        .args(["info", "--query=property", "--name", &node])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

fn apply_udev(p: &mut Pcm, props: &str) {
    let get = |k: &str| props.lines().find_map(|l| l.strip_prefix(k)?.strip_prefix('=')).map(str::to_string);
    p.label = get("OHC_AUDIO_LABEL").filter(|s| !s.is_empty());
    p.swap = get("OHC_AUDIO_SWAP").as_deref() == Some("1");
}

/// A device-tree node's string property.
fn dt_str(node: &Path, prop: &str) -> Option<String> {
    let b = std::fs::read(node.join(prop)).ok()?;
    let s = String::from_utf8_lossy(&b).trim_end_matches('\0').to_string();
    (!s.is_empty()).then_some(s)
}

/// The node a phandle points at, searched from `root`.
fn by_phandle(root: &Path, ph: u32) -> Option<PathBuf> {
    let read = |d: &Path| std::fs::read(d.join("phandle")).ok().filter(|b| b.len() == 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
    if read(root) == Some(ph) {
        return Some(root.to_path_buf());
    }
    std::fs::read_dir(root).ok()?.flatten().filter(|e| e.path().is_dir() && !e.path().is_symlink()).find_map(|e| by_phandle(&e.path(), ph))
}

/// The first phandle in a `sound-dai` property.
fn sound_dai(node: &Path) -> Option<u32> {
    let b = std::fs::read(node.join("sound-dai")).ok()?;
    (b.len() >= 4).then(|| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// A simple-audio-card's codec node per link, in PCM device order.
pub fn card_codecs(card: &Path, root: &Path) -> Vec<Option<PathBuf>> {
    let codec_of = |link: &Path, name: &str| sound_dai(&link.join(name)).and_then(|ph| by_phandle(root, ph));
    let mut links: Vec<PathBuf> = std::fs::read_dir(card)
        .map(|r| r.flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("simple-audio-card,dai-link"))).collect())
        .unwrap_or_default();
    if links.is_empty() {
        return vec![codec_of(card, "simple-audio-card,codec")];
    }
    // dai-link@N: simple-card walks them in tree order, which is unit-address order.
    let unit = |p: &PathBuf| p.file_name().and_then(|n| n.to_str()?.split_once('@')?.1.parse::<u32>().ok()).unwrap_or(u32::MAX);
    links.sort_by_key(|p| (unit(p), p.clone()));
    links.iter().map(|l| codec_of(l, "codec")).collect()
}

/// The JSON profile beside a codec's `firmware-name`, if it is installed.
pub fn dsp_profile(codec: &Path, dir: &Path) -> Option<String> {
    let fw = dt_str(codec, "firmware-name")?;
    let stem = Path::new(&fw).file_stem()?.to_string_lossy().into_owned();
    let json = dir.join(format!("{stem}.json"));
    json.exists().then(|| json.display().to_string())
}

fn apply_dsp(pcms: &mut [Pcm]) {
    let root = Path::new("/sys/firmware/devicetree/base");
    let mut cards: Vec<u32> = pcms.iter().map(|p| p.card).collect();
    cards.dedup();
    for c in cards {
        let Ok(node) = std::fs::canonicalize(format!("/sys/class/sound/card{c}/device/of_node")) else { continue };
        for (dev, codec) in card_codecs(&node, root).into_iter().enumerate() {
            let dsp = codec.and_then(|n| dsp_profile(&n, Path::new(DSP_DIR)));
            for p in pcms.iter_mut().filter(|p| p.card == c && p.device == dev as u32 && !p.capture) {
                p.dsp = dsp.clone();
            }
        }
    }
}

/// The format to run a device at from its `aplay --dump-hw-params` output:
/// S16_LE if it takes it, else S32_LE.
pub fn format_of(dump: &str) -> Option<String> {
    let line = dump.lines().find_map(|l| l.trim().strip_prefix("FORMAT:"))?;
    let has = |f: &str| line.split_whitespace().any(|t| t == f);
    ["S16_LE", "S32_LE"].into_iter().find(|f| has(f)).map(str::to_string)
}

/// Ask the hardware; it is busy only for a moment while audiod restarts.
/// Capture probes with `aplay -C`, bounded by `-d 1`.
fn probe_format(device: &str, capture: bool) -> Option<String> {
    let mode: &[&str] = if capture { &["-C", "-d", "1"] } else { &[] };
    for _ in 0..5 {
        let out = std::process::Command::new("aplay")
            .args(mode)
            .args(["--dump-hw-params", "-D", device, "-t", "raw", "/dev/null"])
            .output()
            .ok()?;
        if let Some(f) = format_of(&String::from_utf8_lossy(&out.stderr)) {
            return Some(f);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    eprintln!("audiod: cannot read {device}'s formats; assuming S16_LE");
    None
}

/// `Control4 EA-3 v1` -> `EA-3`: the model without its vendor and variant.
pub fn product(model: &str) -> String {
    // A device-tree model leads with the vendor.
    let words: Vec<&str> = model.split_whitespace().skip(1).collect();
    let variant = |w: &&str| w.starts_with('(') || (w.len() > 1 && w[..1].eq_ignore_ascii_case("v") && w[1..].starts_with(|c: char| c.is_ascii_digit()));
    let end = words.iter().position(variant).unwrap_or(words.len());
    let p = words[..end].join(" ");
    if p.is_empty() { "openHC".into() } else { p }
}

impl Port {
    /// The ALSA card behind the device, for its mixer controls:
    /// `hw:CARD=Intel,DEV=2` → `Intel`, `hw:1,0` → `1`.
    pub fn card(&self) -> String {
        let spec = self.device.split_once(':').map_or("", |(_, s)| s);
        let first = spec.split(',').next().unwrap_or("");
        let card = spec
            .split(',')
            .find_map(|kv| kv.strip_prefix("CARD="))
            .unwrap_or(if first.contains('=') { "0" } else { first });
        if card.is_empty() { "0".into() } else { card.to_string() }
    }
}

impl Board {
    pub fn discover() -> Board {
        let mut pcms = pcms(Path::new(ASOUND));
        for p in &mut pcms {
            let props = udev(p);
            apply_udev(p, &props);
        }
        apply_dsp(&mut pcms);
        let mut outputs = ports(&pcms, false, "ohc_");
        for o in &mut outputs {
            o.format = probe_format(&o.device, false);
        }
        let mut inputs = ports(&pcms, true, "ohc_in_");
        for i in &mut inputs {
            i.format = probe_format(&i.device, true);
        }
        let model = std::fs::read("/proc/device-tree/model").map(|m| String::from_utf8_lossy(&m).trim_end_matches('\0').to_string()).unwrap_or_default();
        Board {
            outputs,
            inputs,
            rate: env("OHC_AUDIO_RATE").parse().unwrap_or(44100),
            helpers: env("OHC_AUDIO_HELPERS").split_whitespace().map(str::to_string).collect(),
            product: product(&model),
        }
    }
    pub fn has_map(&self) -> bool {
        !self.outputs.is_empty()
    }
    pub fn output(&self, id: &str) -> Option<&Port> {
        self.outputs.iter().find(|p| p.id == id)
    }
    pub fn input(&self, id: &str) -> Option<&Port> {
        self.inputs.iter().find(|p| p.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("audiod-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn card_of_a_device() {
        let p = |d: &str| Port { id: "x".into(), label: "x".into(), device: d.into(), pcm: "ohc_x".into(), ..Default::default() };
        assert_eq!(p("hw:CARD=Intel,DEV=2").card(), "Intel");
        assert_eq!(p("hw:1,0").card(), "1");
        assert_eq!(p("hw:DEV=3,CARD=PCH").card(), "PCH");
        assert_eq!(p("default").card(), "0");
    }

    #[test]
    fn asoc_names_lose_the_codec_dai() {
        assert_eq!(label_of("Analog 1 snd-soc-dummy-dai-0", 0), "Analog 1");
        assert_eq!(label_of("Analog adau1451-ch0-2", 2), "Analog");
        assert_eq!(label_of("HDMI", 0), "HDMI");
        assert_eq!(label_of("ALC888-VD Alt Analog", 2), "ALC888-VD Alt Analog");
        assert_eq!(label_of("Line in -1", 1), "Line in -1");
    }

    #[test]
    fn ids_are_the_labels_slugged() {
        assert_eq!(slug("Analog 1"), "analog1");
        assert_eq!(slug("Coax (S/PDIF)"), "coax");
        assert_eq!(slug("Line in 2"), "linein2");
        assert_eq!(slug("HDMI"), "hdmi");
    }

    #[test]
    fn reads_proc_asound() {
        let d = tmp("asound");
        let pcm = |card: &str, id: &str, s: &str, name: &str| {
            let p = d.join(card).join(s);
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(d.join(card).join("id"), format!("{id}\n")).unwrap();
            std::fs::write(p.join("info"), format!("card: 0\ndevice: 0\nid: {name}\nname: \nsubname: subdevice #0\n")).unwrap();
        };
        pcm("card1", "HDMI", "pcm0p", "HDMI");
        pcm("card0", "openhcea5", "pcm1p", "Coax 1 snd-soc-dummy-dai-1");
        pcm("card0", "openhcea5", "pcm1c", "Coax 1 snd-soc-dummy-dai-1");
        pcm("card0", "openhcea5", "pcm0p", "Analog 1 snd-soc-dummy-dai-0");
        std::fs::write(d.join("cards"), "").unwrap();
        let ps = pcms(&d);
        std::fs::remove_dir_all(&d).unwrap();
        let got: Vec<_> = ps.iter().map(|p| (p.card, p.card_id.as_str(), p.device, p.capture)).collect();
        assert_eq!(got, [(0, "openhcea5", 0, false), (0, "openhcea5", 1, false), (0, "openhcea5", 1, true), (1, "HDMI", 0, false)]);

        let outs = ports(&ps, false, "ohc_");
        let o: Vec<_> = outs.iter().map(|p| (p.id.as_str(), p.label.as_str(), p.device.as_str(), p.pcm.as_str())).collect();
        assert_eq!(o, [
            ("analog1", "Analog 1", "hw:CARD=openhcea5,DEV=0", "ohc_analog1"),
            ("coax1", "Coax 1", "hw:CARD=openhcea5,DEV=1", "ohc_coax1"),
            ("hdmi", "HDMI", "hw:CARD=HDMI,DEV=0", "ohc_hdmi"),
        ]);
        let ins = ports(&ps, true, "ohc_in_");
        assert_eq!((ins[0].id.as_str(), ins[0].pcm.as_str()), ("coax1", "ohc_in_coax1"));
    }

    #[test]
    fn a_udev_labelled_card_shows_only_its_labelled_pcms() {
        // The HC-800's HDA: four playback PCMs and two capture, of which the
        // board's rule names the jacks.
        let hda = |device, capture, name: &str| Pcm { card: 0, card_id: "Intel".into(), device, capture, name: name.into(), ..Pcm::default() };
        let mut ps = vec![
            hda(0, false, "ALC888-VD Analog"), hda(1, false, "ALC888-VD Alt Digital"),
            hda(2, false, "ALC888-VD Alt Analog"), hda(3, false, "ALC888-VD Digital"),
            hda(0, true, "ALC888-VD Analog"), hda(2, true, "ALC888-VD Alt Analog"),
        ];
        for (i, props) in ["OHC_AUDIO_LABEL=Analog 1", "OHC_AUDIO_LABEL=Coax", "OHC_AUDIO_LABEL=Analog 2\nOHC_AUDIO_SWAP=1",
                           "OHC_AUDIO_LABEL=HDMI", "OHC_AUDIO_LABEL=Line in", ""].iter().enumerate() {
            apply_udev(&mut ps[i], props);
        }
        let outs = ports(&ps, false, "ohc_");
        let o: Vec<_> = outs.iter().map(|p| (p.id.as_str(), p.device.as_str(), p.swap)).collect();
        assert_eq!(o, [
            ("analog1", "hw:CARD=Intel,DEV=0", false),
            ("analog2", "hw:CARD=Intel,DEV=2", true),
            ("coax", "hw:CARD=Intel,DEV=1", false),
            ("hdmi", "hw:CARD=Intel,DEV=3", false),
        ]);
        let ins = ports(&ps, true, "ohc_in_");
        assert_eq!(ins.len(), 1);
        assert_eq!((ins[0].id.as_str(), ins[0].label.as_str()), ("linein", "Line in"));
    }

    #[test]
    fn labelling_only_the_inputs_leaves_the_outputs_alone() {
        // A duplex link (the EA-5's FPGA card) names both directions alike; a
        // rule can name the capture side on its own.
        let link = |device, capture, name: &str| Pcm { card: 0, card_id: "openhcea5".into(), device, capture, name: name.into(), ..Pcm::default() };
        let mut ps = vec![link(0, false, "Analog 1 snd-soc-dummy-dai-0"), link(0, true, "Analog 1 snd-soc-dummy-dai-0")];
        apply_udev(&mut ps[1], "OHC_AUDIO_LABEL=Line in 1");
        assert_eq!(ports(&ps, false, "ohc_")[0].id, "analog1");
        assert_eq!(ports(&ps, true, "ohc_in_")[0].id, "linein1");
    }

    #[test]
    fn colliding_labels_get_distinct_ids() {
        let p = |card, id: &str| Pcm { card, card_id: id.into(), name: "HDMI".into(), ..Pcm::default() };
        let outs = ports(&[p(0, "HDMI"), p(1, "NVidia")], false, "ohc_");
        let ids: Vec<_> = outs.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["hdmi", "hdmi2"]);
    }

    #[test]
    fn the_dsp_profile_follows_the_codecs_firmware() {
        let d = tmp("dt");
        let dsp = d.join("i2c/audio-codec@38");
        std::fs::create_dir_all(&dsp).unwrap();
        std::fs::write(dsp.join("phandle"), 7u32.to_be_bytes()).unwrap();
        std::fs::write(dsp.join("firmware-name"), b"openhc/adau1451-ea3-openhc.bin\0").unwrap();
        let card = d.join("sound");
        std::fs::create_dir_all(card.join("simple-audio-card,codec")).unwrap();
        std::fs::write(card.join("simple-audio-card,codec/sound-dai"), 7u32.to_be_bytes()).unwrap();
        let codecs = card_codecs(&card, &d);
        assert_eq!(codecs, [Some(dsp.clone())]);
        let share = d.join("share");
        std::fs::create_dir_all(&share).unwrap();
        assert_eq!(dsp_profile(&dsp, &share), None);
        std::fs::write(share.join("adau1451-ea3-openhc.json"), "{}").unwrap();
        assert_eq!(dsp_profile(&dsp, &share), Some(share.join("adau1451-ea3-openhc.json").display().to_string()));

        // Several links: one codec each, in unit-address order.
        let multi = d.join("sound2");
        for (link, ph) in [("simple-audio-card,dai-link@1", 9u32), ("simple-audio-card,dai-link@0", 7)] {
            std::fs::create_dir_all(multi.join(link).join("codec")).unwrap();
            std::fs::write(multi.join(link).join("codec/sound-dai"), ph.to_be_bytes()).unwrap();
        }
        assert_eq!(card_codecs(&multi, &d), [Some(dsp), None]);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn the_format_is_s16_where_the_hardware_takes_it() {
        let dump = |f: &str| format!("HW Params of device \"hw:CARD=HDMI,DEV=0\":\n--------------------\nACCESS:  MMAP_INTERLEAVED RW_INTERLEAVED\nFORMAT:  {f}\nSUBFORMAT:  STD\n");
        assert_eq!(format_of(&dump("S16_LE S32_LE")).as_deref(), Some("S16_LE"));
        assert_eq!(format_of(&dump("S32_LE")).as_deref(), Some("S32_LE"));
        assert_eq!(format_of(&dump("S24_3LE")), None);
        assert_eq!(format_of("aplay: main:834: audio open error: Device or resource busy"), None);
    }

    #[test]
    fn the_product_is_the_model_without_vendor_or_variant() {
        assert_eq!(product("Control4 EA-3 v1"), "EA-3");
        assert_eq!(product("Control4 EA-1 v2 PoE"), "EA-1");
        assert_eq!(product("Control4 EA-5"), "EA-5");
        assert_eq!(product("Control4 HC-800"), "HC-800");
        assert_eq!(product("Control4 CA-1 (i.MX6SL)"), "CA-1");
        assert_eq!(product("Control4 IO Extender V1 (DM355 hammer)"), "IO Extender");
        assert_eq!(product(""), "openHC");
        assert_eq!(product("HC-800"), "openHC");
    }
}
