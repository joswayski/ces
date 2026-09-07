use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;

use crate::{AppState, db, workos::WorkosUser};

const TOLERANCE_MS: i64 = 180_000;

#[derive(Deserialize)]
struct Event {
    id: String,
    #[serde(rename = "event")]
    kind: String,
    data: serde_json::Value,
}
#[derive(Deserialize)]
struct DeletedUser {
    id: String,
    #[serde(default)]
    updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    deleted_at: Option<DateTime<Utc>>,
}

pub async fn handle(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> StatusCode {
    let Some(signature) = headers
        .get("workos-signature")
        .and_then(|v| v.to_str().ok())
    else {
        return StatusCode::UNAUTHORIZED;
    };
    if !verify_signature(signature, &body, &state.config.webhook_secret, now_ms()) {
        return StatusCode::UNAUTHORIZED;
    }
    let Ok(event) = serde_json::from_slice::<Event>(&body) else {
        return StatusCode::BAD_REQUEST;
    };
    let Ok(mut tx) = state.pool.begin().await else {
        return StatusCode::SERVICE_UNAVAILABLE;
    };
    match db::record_event(&mut tx, &event.id, &event.kind).await {
        Ok(false) => {
            return if tx.commit().await.is_ok() {
                StatusCode::OK
            } else {
                StatusCode::SERVICE_UNAVAILABLE
            };
        }
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE,
        Ok(true) => {}
    }
    let result = match event.kind.as_str() {
        "user.updated" => match serde_json::from_value::<WorkosUser>(event.data) {
            Ok(user) => db::event_update(&mut tx, &user).await.map_err(|_| ()),
            Err(_) => Err(()),
        },
        "user.deleted" => match serde_json::from_value::<DeletedUser>(event.data) {
            Ok(user) => db::event_delete(
                &mut tx,
                &user.id,
                user.deleted_at.or(user.updated_at).unwrap_or_else(Utc::now),
            )
            .await
            .map_err(|_| ()),
            Err(_) => Err(()),
        },
        _ => Ok(()),
    };
    if result.is_err() {
        return StatusCode::SERVICE_UNAVAILABLE;
    }
    if tx.commit().await.is_err() {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::OK
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}
pub(crate) fn verify_signature(header: &str, body: &[u8], secret: &str, now: i64) -> bool {
    let mut timestamp = None;
    let mut signatures = Vec::new();
    for part in header.split(',').map(str::trim) {
        if let Some(v) = part.strip_prefix("t=") {
            timestamp = v.parse::<i64>().ok();
        }
        if let Some(v) = part.strip_prefix("v1=") {
            signatures.push(v);
        }
    }
    let Some(ts) = timestamp else { return false };
    if now.abs_diff(ts) > TOLERANCE_MS.unsigned_abs() {
        return false;
    }
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(ts.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    signatures
        .into_iter()
        .filter_map(|s| hex::decode(s).ok())
        .any(|s| mac.clone().verify_slice(&s).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signatures() {
        let body = br#"{"event":"user.updated"}"#;
        let ts = 1_000_000;
        let mut mac = Hmac::<Sha256>::new_from_slice(b"secret").unwrap();
        mac.update(format!("{ts}.").as_bytes());
        mac.update(body);
        let header = format!("t={ts}, v1={}", hex::encode(mac.finalize().into_bytes()));
        assert!(verify_signature(&header, body, "secret", ts));
        assert!(!verify_signature(&header, b"x", "secret", ts));
        assert!(!verify_signature(
            &header,
            body,
            "secret",
            ts + TOLERANCE_MS + 1
        ));
    }
}
