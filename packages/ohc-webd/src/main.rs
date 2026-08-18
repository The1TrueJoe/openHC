//! ohc-webd — openHC controller dashboard + REST API.
//!
//! Board-agnostic: everything it shows comes from /opt/ohc/board.env, and the
//! serial/radio endpoints act on whatever that file declares. Serves the React
//! UI compiled into this binary, plus a WebSocket serial bridge for the in-UI
//! terminal. Single-threaded tokio runtime — this box has one Cortex-A9.
mod api;
mod board;
mod serial;
mod system;

use std::sync::Arc;

pub struct Config {
    pub bind: String,
    pub board_env: String,
}

fn parse_args() -> Config {
    let mut cfg = Config {
        bind: "0.0.0.0:80".into(),
        board_env: "/opt/ohc/board.env".into(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--bind" => cfg.bind = args.next().unwrap_or(cfg.bind),
            "--board-env" => cfg.board_env = args.next().unwrap_or(cfg.board_env),
            "-h" | "--help" => {
                println!("ohc-webd [--bind ADDR:PORT] [--board-env PATH]");
                std::process::exit(0);
            }
            _ => {}
        }
    }
    cfg
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let cfg = Arc::new(parse_args());
    let app = api::router(cfg.clone());
    let listener = tokio::net::TcpListener::bind(&cfg.bind)
        .await
        .unwrap_or_else(|e| {
            eprintln!("ohc-webd: cannot bind {}: {e}", cfg.bind);
            std::process::exit(1);
        });
    eprintln!("ohc-webd: listening on {}", cfg.bind);
    let shutdown = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    if let Err(e) = axum::serve(listener, app).with_graceful_shutdown(shutdown).await {
        eprintln!("ohc-webd: {e}");
    }
}
