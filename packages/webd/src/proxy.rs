//! `/iod/*` and `/sys/*` — reverse proxies onto the other daemons.
//!
//! iod listens on its own port because it is a separate process with a
//! different owner: it holds the UARTs, webd holds the filesystem. That is the
//! right split for the daemons, but it is a bad thing to expose to a BROWSER.
//! A page served from :80 that fetches :7070 needs both ports reachable from
//! wherever the operator is sitting, and one restrictive firewall, corporate
//! proxy or port-filtering client turns the entire config GUI into an error
//! card — while the box itself is perfectly healthy.
//!
//! So the GUI talks to ONE origin and webd forwards. The extra hop is loopback
//! on the same machine, which is nothing next to a UART running at 115200 baud,
//! and it buys three things: the GUI works anywhere the page can load at all,
//! cross-origin rules stop applying, and there is a single place to
//! authenticate rather than two.
//!
//! WebSockets are forwarded too, which is the part that makes this a proxy
//! rather than a URL rewrite: the serial terminal and the control socket are
//! upgrades, and an upgrade has to be spliced, not buffered.
use axum::{
    body::Body,
    extract::Request,
    http::{header, HeaderValue, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;

/// Where iod listens. Loopback deliberately: this proxy is for the browser's
/// benefit, not a way to reach another machine's IO.
pub fn iod_addr() -> String {
    std::env::var("WEBD_IOD_ADDR").unwrap_or_else(|_| "127.0.0.1:7070".into())
}

/// Where sysmond listens — telemetry, separate daemon, separate port.
pub fn sysmond_addr() -> String {
    std::env::var("WEBD_SYSMOND_ADDR").unwrap_or_else(|_| "127.0.0.1:7071".into())
}

/// One proxy, two mounts. Which daemon a request goes to is decided by the
/// prefix alone, so adding a third is a line here and a route in main.
pub async fn handler(mut req: Request) -> Response {
    let is_sys = req.uri().path().starts_with("/sys/");
    let addr = if is_sys { sysmond_addr() } else { iod_addr() };
    let mount = if is_sys { "/sys" } else { "/iod" };
    let who = if is_sys { "sysmond" } else { "iod" };

    // Strip the mount point: /iod/api/io upstream is /api/io. `/mqtt` is
    // mounted at the same path upstream, so it passes through untouched — the
    // browser's MQTT client connects to ws://<this host>/mqtt and never learns
    // that iod is a separate process on another port.
    let path = req.uri().path();
    let rest = path.strip_prefix(mount).unwrap_or(path);
    let rest = if rest.is_empty() { "/" } else { rest };
    let q = req.uri().query().map(|q| format!("?{q}")).unwrap_or_default();
    *req.uri_mut() = match format!("{rest}{q}").parse::<Uri>() {
        Ok(u) => u,
        Err(_) => return (StatusCode::BAD_REQUEST, "bad path").into_response(),
    };
    // A forwarded request must carry the UPSTREAM's authority, not ours.
    req.headers_mut().insert(header::HOST, HeaderValue::from_str(&addr).unwrap());

    let stream = match TcpStream::connect(&addr).await {
        Ok(s) => s,
        // The common case in practice: iod is not running. Say which service is
        // missing, because "502" on a page served by a working webd is a
        // genuinely confusing thing to debug.
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                axum::Json(serde_json::json!({
                    "error": format!("cannot reach {who} at {addr}: {e}"),
                    "code": format!("{who}_unreachable"),
                })),
            )
                .into_response()
        }
    };

    let (mut sender, conn) = match hyper::client::conn::http1::handshake(TokioIo::new(stream)).await {
        Ok(p) => p,
        Err(e) => return (StatusCode::BAD_GATEWAY, format!("iod handshake failed: {e}")).into_response(),
    };
    // `with_upgrades` keeps the connection usable after a 101 instead of
    // dropping it, which is what lets the serial socket survive.
    tokio::spawn(async move {
        let _ = conn.with_upgrades().await;
    });

    // Claim the client side of the upgrade BEFORE the request is consumed.
    // After `send_request` the original is gone, and with it the only handle to
    // the downstream half of a future WebSocket.
    let downstream = hyper::upgrade::on(&mut req);

    let mut res = match sender.send_request(req).await {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_GATEWAY, format!("iod refused the request: {e}")).into_response(),
    };

    if res.status() == StatusCode::SWITCHING_PROTOCOLS {
        let upstream_up = hyper::upgrade::on(&mut res);
        // Both halves are only available once each side has actually switched,
        // so the copy waits on them rather than running now.
        tokio::spawn(async move {
            match tokio::try_join!(downstream, upstream_up) {
                Ok((d, u)) => {
                    let (mut d, mut u) = (TokioIo::new(d), TokioIo::new(u));
                    // Raw bytes from here on. Neither side is parsed again —
                    // a proxy that reframed WebSocket traffic would be a
                    // proxy that could corrupt a serial stream.
                    let _ = tokio::io::copy_bidirectional(&mut d, &mut u).await;
                }
                Err(e) => eprintln!("webd: iod upgrade failed: {e}"),
            }
        });
    }

    res.map(Body::new).into_response()
}
