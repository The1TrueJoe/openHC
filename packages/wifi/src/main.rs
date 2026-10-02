//! openHC captive-portal Wi-Fi setup — a standalone web app, deliberately kept
//! out of the dashboard (webd). S41wifi-ap starts it only while the setup AP
//! is up and stops it when the AP goes down, so it can serve the setup page for
//! EVERY path (that blanket response is what trips the phone's OS captive-portal
//! check) without ever interfering with the dashboard. Joining hands off to the
//! shared wifi crate, which writes the station config and restarts the AP
//! script to switch wlan from AP to station. Listens on :80 by default.
use axum::{
    extract::Json,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let bind = std::env::args().nth(1).unwrap_or_else(|| "0.0.0.0:80".into());
    let app = Router::new()
        .route("/api/wifi/scan", get(scan))
        .route("/api/wifi/connect", post(connect))
        .fallback(portal);
    let listener = tokio::net::TcpListener::bind(&bind).await.unwrap_or_else(|e| {
        eprintln!("portal: cannot bind {bind}: {e}");
        std::process::exit(1);
    });
    eprintln!("portal: captive portal on {bind}");
    if let Err(e) = axum::serve(listener, app).await {
        eprintln!("portal: {e}");
    }
}

async fn scan() -> Json<Vec<String>> {
    Json(wifi::scan_cache())
}

#[derive(serde::Deserialize)]
struct ConnectBody {
    ssid: String,
    psk: Option<String>,
}

async fn connect(Json(b): Json<ConnectBody>) -> Response {
    let iface = wifi::wifi_iface();
    match wifi::apply(&iface, b.ssid.trim(), b.psk.as_deref().unwrap_or("").trim()) {
        Ok(ssid) => Json(serde_json::json!({ "joining": ssid })).into_response(),
        Err(e) => {
            (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": e }))).into_response()
        }
    }
}

async fn portal() -> Response {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        wifi::PORTAL_HTML,
    )
        .into_response()
}
