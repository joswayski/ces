use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use jsonwebtoken::{
    Algorithm, DecodingKey, Validation, decode, decode_header,
    jwk::{Jwk, JwkSet},
};
use serde::Deserialize;
use tokio::sync::Mutex;

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("invalid token")]
    Invalid,
    #[error("identity service unavailable")]
    Unavailable,
}

#[derive(Clone)]
pub struct JwtVerifier {
    client: reqwest::Client,
    url: String,
    issuer: String,
    audience: String,
    cache: Arc<Mutex<Cache>>,
}
struct Cache {
    keys: Vec<Jwk>,
    fetched: Option<Instant>,
    last_refresh_attempt: Option<Instant>,
}

#[derive(Deserialize)]
struct Claims {
    sub: String,
    sid: String,
    iss: String,
    exp: u64,
    #[serde(default)]
    aud: Option<serde_json::Value>,
}

impl JwtVerifier {
    pub fn new(client: reqwest::Client, api_base: &str, client_id: &str, issuer: &str) -> Self {
        Self {
            client,
            url: format!("{api_base}/sso/jwks/{client_id}"),
            issuer: issuer.into(),
            audience: client_id.into(),
            cache: Arc::new(Mutex::new(Cache {
                keys: vec![],
                fetched: None,
                last_refresh_attempt: None,
            })),
        }
    }

    pub async fn verify(&self, token: &str) -> Result<String, AuthError> {
        let header = decode_header(token).map_err(|_| AuthError::Invalid)?;
        if header.alg != Algorithm::RS256 {
            return Err(AuthError::Invalid);
        }
        let kid = header.kid.ok_or(AuthError::Invalid)?;
        let key = self.key(&kid).await?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.leeway = 0;
        validation.set_issuer(&[&self.issuer]);
        validation.set_required_spec_claims(&["exp", "iss", "sub", "sid"]);
        validation.validate_aud = false;
        let claims = decode::<Claims>(token, &key, &validation)
            .map_err(|_| AuthError::Invalid)?
            .claims;
        if claims.sub.is_empty()
            || claims.sid.is_empty()
            || claims.iss != self.issuer
            || claims.exp == 0
            || !audience_matches(claims.aud.as_ref(), &self.audience)
        {
            return Err(AuthError::Invalid);
        }
        Ok(claims.sub)
    }

    async fn key(&self, kid: &str) -> Result<DecodingKey, AuthError> {
        let mut cache = self.cache.lock().await;
        let stale = cache
            .fetched
            .is_none_or(|at| at.elapsed() > Duration::from_secs(3600));
        let found = cache
            .keys
            .iter()
            .find(|k| k.common.key_id.as_deref() == Some(kid));
        if !stale && let Some(found) = found {
            return DecodingKey::from_jwk(found).map_err(|_| AuthError::Invalid);
        }
        if let Some(at) = cache.last_refresh_attempt
            && at.elapsed() < Duration::from_secs(10)
        {
            // An unavailable JWKS is not evidence that the user's token is invalid.
            return Err(
                if stale || cache.fetched.is_none_or(|fetched| fetched < at) {
                    AuthError::Unavailable
                } else {
                    AuthError::Invalid
                },
            );
        }
        cache.last_refresh_attempt = Some(Instant::now());
        let response = self
            .client
            .get(&self.url)
            .send()
            .await
            .map_err(|_| AuthError::Unavailable)?
            .error_for_status()
            .map_err(|_| AuthError::Unavailable)?;
        let set: JwkSet = response.json().await.map_err(|_| AuthError::Unavailable)?;
        if set.keys.len() > 16 {
            return Err(AuthError::Unavailable);
        }
        cache.keys = set.keys;
        cache.fetched = Some(Instant::now());
        cache
            .keys
            .iter()
            .find(|k| k.common.key_id.as_deref() == Some(kid))
            .ok_or(AuthError::Invalid)
            .and_then(|k| DecodingKey::from_jwk(k).map_err(|_| AuthError::Invalid))
    }
}

fn audience_matches(aud: Option<&serde_json::Value>, expected: &str) -> bool {
    match aud {
        None => true,
        Some(serde_json::Value::String(v)) => v == expected,
        Some(serde_json::Value::Array(v)) => v.iter().any(|x| x.as_str() == Some(expected)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audience_is_optional_but_must_match_when_present() {
        assert!(audience_matches(None, "client"));
        assert!(audience_matches(
            Some(&serde_json::json!("client")),
            "client"
        ));
        assert!(audience_matches(
            Some(&serde_json::json!(["other", "client"])),
            "client"
        ));
        assert!(!audience_matches(
            Some(&serde_json::json!("other")),
            "client"
        ));
    }
}
