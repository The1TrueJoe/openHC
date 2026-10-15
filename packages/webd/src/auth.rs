//! The controller's one shared secret, enforced at the front door.
//!
//! `IOD_TOKEN` (set in /etc/openhc/iod.conf) has always protected iod's REST.
//! The other daemons — sysmond, ohc-audiod, ohc-storaged — listen on loopback
//! only and are reached through this webd, so the token has to be checked here
//! or it protects nothing but the IO: the drive files, the audio map and webd's
//! own Wi-Fi endpoints would stay open to the LAN.
//!
//! Same rules as iod (packages/iod/src/api.rs `auth`): off unless the token is
//! set; `Authorization: Bearer <token>` or `?token=` (a browser WebSocket
//! cannot send a header); liveness (`…/api/health`) and CORS preflights stay
//! open. `/iod/*` is not checked here — iod checks it itself.
use axum::{
    extract::Request,
    http::{header, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

/// Whether `req` carries `want`.
fn presented(req: &Request, want: &str) -> bool {
    let header = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_string);
    let query = req
        .uri()
        .query()
        .unwrap_or("")
        .split('&')
        .find_map(|kv| kv.strip_prefix("token=").map(|v| v.replace("%20", " ")));
    constant_eq(header.or(query).unwrap_or_default().as_bytes(), want.as_bytes())
}

pub async fn require(req: Request, next: Next) -> Response {
    let want = std::env::var("IOD_TOKEN").unwrap_or_default();
    if want.is_empty()
        || req.method() == Method::OPTIONS
        || req.uri().path().ends_with("/api/health")
        || presented(&req, &want)
    {
        return next.run(req).await;
    }
    (StatusCode::UNAUTHORIZED, Json(json!({ "error": "bad or missing token", "code": "unauthorized" })))
        .into_response()
}

/// Compare without an early return, so the time taken does not reveal how much
/// of a guessed token was right.
fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    fn req(uri: &str, bearer: Option<&str>) -> Request {
        let mut b = axum::http::Request::builder().uri(uri);
        if let Some(t) = bearer {
            b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        b.body(axum::body::Body::empty()).unwrap()
    }
    #[test]
    fn accepts_the_header_or_the_query() {
        assert!(presented(&req("/storage/api/files?path=x", Some("s3cret")), "s3cret"));
        assert!(presented(&req("/storage/api/files?path=x&token=s3cret", None), "s3cret"));
        assert!(!presented(&req("/storage/api/files?path=x&token=wrong", None), "s3cret"));
        assert!(!presented(&req("/storage/api/files", None), "s3cret"));
        assert!(!constant_eq(b"abc", b"abcd"));
    }
}
