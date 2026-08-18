//! Shared Wi-Fi setup helpers, used by two separate binaries: the captive-portal
//! app (ohc-portal, serves the setup page while the AP is up) and the dashboard
//! (ohc-webd, exposes the same control over its API). Neither does wireless I/O
//! directly — the shared S41wifi-ap script drops scanned SSIDs into
//! /tmp/ohc-wifi-scan, and joining here writes a wpa_supplicant station config
//! then kicks that script to switch wlan from AP to station.
const SCAN_CACHE: &str = "/tmp/ohc-wifi-scan";
const BOARD_ENV: &str = "/opt/ohc/board.env";

/// The board's Wi-Fi interface (OHC_WIFI_IFACE in board.env), empty if none.
pub fn wifi_iface() -> String {
    std::fs::read_to_string(BOARD_ENV)
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.trim().strip_prefix("OHC_WIFI_IFACE="))
        .map(|v| v.split('#').next().unwrap_or("").trim().trim_matches('"').trim_matches('\'').to_string())
        .unwrap_or_default()
}

/// SSIDs captured by S41wifi-ap at AP start (empty if the scan found nothing).
pub fn scan_cache() -> Vec<String> {
    std::fs::read_to_string(SCAN_CACHE)
        .unwrap_or_default()
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// wpa_supplicant quoted-string escaping, dropping control chars so a crafted
/// SSID/password can't inject extra config lines.
fn wpa_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().filter(|c| !c.is_control()) {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c => out.push(c),
        }
    }
    out
}

/// Write the station config for `iface` and kick S41wifi-ap to join. Returns the
/// SSID on success. The AP is torn down by that restart (after this response
/// flushes), so the client that submitted this necessarily loses the connection.
pub fn apply(iface: &str, ssid: &str, psk: &str) -> Result<String, String> {
    if ssid.is_empty() {
        return Err("ssid required".into());
    }
    if iface.is_empty() {
        return Err("this board has no wifi radio".into());
    }
    let net = if psk.is_empty() {
        format!("network={{\n\tssid=\"{}\"\n\tkey_mgmt=NONE\n}}\n", wpa_str(ssid))
    } else {
        format!("network={{\n\tssid=\"{}\"\n\tpsk=\"{}\"\n}}\n", wpa_str(ssid), wpa_str(psk))
    };
    let conf = format!("ctrl_interface=/var/run/wpa_supplicant\nupdate_config=1\n{net}");
    std::fs::create_dir_all("/etc/wpa_supplicant").ok();
    let path = format!("/etc/wpa_supplicant/wpa_supplicant-{iface}.conf");
    std::fs::write(&path, conf).map_err(|e| e.to_string())?;
    // Delay so this HTTP response reaches the phone before the AP drops.
    std::process::Command::new("sh")
        .arg("-c")
        .arg("sleep 2; /etc/init.d/S41wifi-ap restart >/dev/null 2>&1")
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(ssid.to_string())
}

/// Self-contained setup page (no React, no assets) — served for every path while
/// the AP is up, which is what trips the OS captive-portal check.
pub const PORTAL_HTML: &str = r####"<!doctype html><html lang=en><head>
<meta charset=utf-8><meta name=viewport content="width=device-width,initial-scale=1">
<title>openHC · Wi-Fi setup</title><style>
:root{color-scheme:dark}
*{box-sizing:border-box}
body{margin:0;min-height:100vh;font:16px/1.5 -apple-system,system-ui,"Segoe UI",Roboto,sans-serif;
  background:#0c111d;color:#e6e9ef;display:flex;justify-content:center;padding:28px 18px}
main{width:100%;max-width:440px}
.eyebrow{font:600 10px/1 ui-monospace,monospace;letter-spacing:.14em;text-transform:uppercase;color:#5b6472;margin:0 0 10px}
h1{font-size:26px;font-weight:700;margin:0 0 8px;letter-spacing:-.01em}
.sub{color:#9aa3b2;margin:0 0 22px;font-size:15px}
.nets{display:flex;flex-direction:column;gap:6px;margin:0 0 22px}
.net{text-align:left;background:#141b2b;border:1px solid #202a3d;color:#e6e9ef;padding:12px 14px;border-radius:10px;
  font-size:15px;cursor:pointer}
.net:hover,.net:focus{border-color:#10b981}
form{display:flex;flex-direction:column;gap:14px}
label{display:flex;flex-direction:column;gap:6px;font-size:13px;color:#9aa3b2}
input{background:#0f1626;border:1px solid #202a3d;color:#e6e9ef;padding:13px 14px;border-radius:10px;font-size:16px}
input:focus{outline:none;border-color:#10b981}
button[type=submit]{margin-top:4px;background:#10b981;border:0;color:#04120c;font-weight:700;font-size:16px;
  padding:14px;border-radius:10px;cursor:pointer}
button[type=submit]:disabled{opacity:.6;cursor:default}
.msg{margin-top:18px;padding:14px;border-radius:10px;font-size:15px;display:none}
.msg.ok{display:block;background:#0d2a20;border:1px solid #10b981;color:#a7f3d0}
.msg.err{display:block;background:#2a0f12;border:1px solid #f87171;color:#fecaca}
</style></head><body><main>
<div class=eyebrow>openHC setup</div>
<h1>Connect to Wi-Fi</h1>
<p class=sub>Pick your network and enter its password. openHC will join it, and this setup hotspot will close.</p>
<div id=nets class=nets></div>
<form id=f>
  <label>Network<input id=ssid autocomplete=off placeholder="Wi-Fi name (SSID)" required></label>
  <label>Password<input id=psk type=password autocomplete=off placeholder="leave blank if the network is open"></label>
  <button type=submit id=go>Join network</button>
</form>
<div id=msg class=msg></div>
</main><script>
var $=function(s){return document.querySelector(s)};
fetch('/api/wifi/scan').then(function(r){return r.json()}).then(function(list){
  if(!list.length)return;var box=$('#nets');
  box.innerHTML='<div class=eyebrow>Networks nearby</div>';
  list.forEach(function(s){var b=document.createElement('button');b.type='button';b.className='net';b.textContent=s;
    b.onclick=function(){$('#ssid').value=s;$('#psk').focus()};box.appendChild(b)});
}).catch(function(){});
$('#f').onsubmit=function(e){e.preventDefault();
  var ssid=$('#ssid').value.trim();if(!ssid)return;
  $('#go').disabled=true;$('#go').textContent='Joining…';
  fetch('/api/wifi/connect',{method:'POST',headers:{'content-type':'application/json'},
    body:JSON.stringify({ssid:ssid,psk:$('#psk').value})})
  .then(function(r){return r.json().then(function(j){return{ok:r.ok,j:j}})})
  .then(function(res){if(!res.ok)throw new Error(res.j.error||'failed');
    $('#msg').className='msg ok';
    $('#msg').innerHTML='Joining <b>'+ssid.replace(/[<>&]/g,'')+'</b>…<br>This hotspot will now close. Reconnect your phone to your home Wi-Fi — openHC will be on it shortly.';
    $('#f').style.display='none';
  }).catch(function(err){$('#msg').className='msg err';$('#msg').textContent=err.message;
    $('#go').disabled=false;$('#go').textContent='Join network'});
};
</script></body></html>"####;
