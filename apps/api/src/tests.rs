use std::{
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{
    Json, Router,
    body::Body,
    http::{Request, StatusCode, header},
    routing::get,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{Duration as ChronoDuration, Utc};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rsa::{
    RsaPrivateKey, RsaPublicKey,
    pkcs8::{EncodePrivateKey, LineEnding},
    rand_core::OsRng,
    traits::PublicKeyParts,
};
use serde_json::{Value, json};
use sqlx::{ConnectOptions, PgPool, postgres::PgPoolOptions};
use tokio::net::TcpListener;
use tower::ServiceExt;

use crate::{
    AppState,
    auth::{AuthError, JwtVerifier},
    config::Config,
    db,
    workos::{self, WorkosUser},
};

struct SigningKey {
    encoding: EncodingKey,
    n: String,
    e: String,
}

fn signing_key() -> SigningKey {
    let private = RsaPrivateKey::new(&mut OsRng, 2048).expect("generate test RSA key");
    let public = RsaPublicKey::from(&private);
    let pem = private
        .to_pkcs8_pem(LineEnding::LF)
        .expect("encode test RSA key");
    SigningKey {
        encoding: EncodingKey::from_rsa_pem(pem.as_bytes()).expect("read test RSA key"),
        n: URL_SAFE_NO_PAD.encode(public.n().to_bytes_be()),
        e: URL_SAFE_NO_PAD.encode(public.e().to_bytes_be()),
    }
}

fn claims(issuer: &str, audience: Value) -> Value {
    json!({
        "sub": "user_1",
        "sid": "session_1",
        "iss": issuer,
        "aud": audience,
        "exp": (Utc::now() + ChronoDuration::minutes(5)).timestamp()
    })
}

fn token(key: &SigningKey, kid: &str, claims: &Value) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(kid.into());
    encode(&header, claims, &key.encoding).expect("sign JWT")
}

async fn mock_server(app: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
    let address = listener.local_addr().expect("mock address");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve mock");
    });
    format!("http://{address}")
}

fn jwks(key: &SigningKey, kid: &str) -> Value {
    json!({"keys": [{
        "kty": "RSA", "use": "sig", "alg": "RS256", "kid": kid,
        "n": key.n, "e": key.e
    }]})
}

#[tokio::test]
async fn jwt_verifier_accepts_only_complete_expected_rs256_tokens() {
    let key = signing_key();
    let issuer = "https://issuer.test/";
    let client_id = "client_1";
    let set = jwks(&key, "good");
    let base = mock_server(Router::new().route(
        "/sso/jwks/client_1",
        get(move || {
            let set = set.clone();
            async move { Json(set) }
        }),
    ))
    .await;
    let verifier = JwtVerifier::new(reqwest::Client::new(), &base, client_id, issuer);

    let valid = claims(issuer, json!(client_id));
    assert_eq!(
        verifier.verify(&token(&key, "good", &valid)).await.unwrap(),
        "user_1"
    );

    for field in ["sub", "sid", "exp", "iss"] {
        let mut invalid = valid.clone();
        invalid.as_object_mut().unwrap().remove(field);
        assert!(
            matches!(
                verifier.verify(&token(&key, "good", &invalid)).await,
                Err(AuthError::Invalid)
            ),
            "missing {field} must be rejected"
        );
    }

    for invalid in [
        claims("https://other-issuer.test/", json!(client_id)),
        claims(issuer, json!("other-client")),
        {
            let mut value = claims(issuer, json!(client_id));
            value["exp"] = json!((Utc::now() - ChronoDuration::seconds(1)).timestamp());
            value
        },
    ] {
        assert!(matches!(
            verifier.verify(&token(&key, "good", &invalid)).await,
            Err(AuthError::Invalid)
        ));
    }

    let other_key = signing_key();
    assert!(matches!(
        verifier.verify(&token(&other_key, "good", &valid)).await,
        Err(AuthError::Invalid)
    ));
    let hs = encode(
        &Header::new(Algorithm::HS256),
        &valid,
        &EncodingKey::from_secret(b"not-rsa"),
    )
    .unwrap();
    assert!(matches!(
        verifier.verify(&hs).await,
        Err(AuthError::Invalid)
    ));
}

#[tokio::test]
async fn unknown_kid_refresh_is_rate_limited() {
    let requests = Arc::new(AtomicUsize::new(0));
    let observed = requests.clone();
    let base = mock_server(Router::new().route(
        "/sso/jwks/client",
        get(move || {
            observed.fetch_add(1, Ordering::SeqCst);
            async { Json(json!({"keys": []})) }
        }),
    ))
    .await;
    let verifier = JwtVerifier::new(reqwest::Client::new(), &base, "client", "issuer");
    let key = signing_key();
    let jwt = token(&key, "absent", &claims("issuer", json!("client")));
    assert!(matches!(
        verifier.verify(&jwt).await,
        Err(AuthError::Invalid)
    ));
    assert!(matches!(
        verifier.verify(&jwt).await,
        Err(AuthError::Invalid)
    ));
    assert_eq!(requests.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn jwks_http_failure_and_timeout_are_unavailable() {
    let failure_base = mock_server(Router::new().route(
        "/sso/jwks/client",
        get(|| async { StatusCode::BAD_GATEWAY }),
    ))
    .await;
    let key = signing_key();
    let jwt = token(&key, "key", &claims("issuer", json!("client")));
    let verifier = JwtVerifier::new(reqwest::Client::new(), &failure_base, "client", "issuer");
    assert!(matches!(
        verifier.verify(&jwt).await,
        Err(AuthError::Unavailable)
    ));

    // Backoff must preserve a dependency outage, not incorrectly report a bad login.
    assert!(matches!(
        verifier.verify(&jwt).await,
        Err(AuthError::Unavailable)
    ));

    let timeout_base = mock_server(Router::new().route(
        "/sso/jwks/client",
        get(|| async {
            tokio::time::sleep(Duration::from_millis(200)).await;
            Json(json!({"keys": []}))
        }),
    ))
    .await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(20))
        .build()
        .unwrap();
    let verifier = JwtVerifier::new(client, &timeout_base, "client", "issuer");
    assert!(matches!(
        verifier.verify(&jwt).await,
        Err(AuthError::Unavailable)
    ));
}

#[tokio::test]
async fn fetch_user_requires_the_requested_identity() {
    let base = mock_server(Router::new().route(
        "/user_management/users/requested",
        get(|| async {
            Json(json!({
                "id": "different",
                "email": "person@example.com",
                "email_verified": true,
                "updated_at": Utc::now(),
            }))
        }),
    ))
    .await;
    assert!(
        workos::fetch_user(&reqwest::Client::new(), &base, "key", "requested")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn account_route_without_authorization_is_protected() {
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
        .unwrap();
    let config = Config {
        database_url: "unused".into(),
        api_key: "key".into(),
        client_id: "client".into(),
        webhook_secret: "secret".into(),
        issuer: "issuer".into(),
        bind: "127.0.0.1:0".parse().unwrap(),
        api_base: "http://127.0.0.1:1".into(),
    };
    let client = reqwest::Client::new();
    let state = AppState {
        verifier: JwtVerifier::new(client.clone(), &config.api_base, "client", "issuer"),
        config,
        pool,
        client,
    };
    let response = Router::new()
        .route("/api/account/me", get(crate::me))
        .with_state(state)
        .oneshot(Request::get("/api/account/me").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
}

fn user(id: &str, email: &str, updated_at: chrono::DateTime<Utc>) -> WorkosUser {
    WorkosUser {
        id: id.into(),
        email: email.into(),
        email_verified: true,
        updated_at,
    }
}

async fn database_regressions(pool: &PgPool) -> Result<(), sqlx::Error> {
    let now = Utc::now();
    let same = user("same", "same@example.com", now);
    let (left, right) = tokio::join!(
        db::sync_authenticated(pool, &same),
        db::sync_authenticated(pool, &same)
    );
    assert!(left?.is_some() && right?.is_some());
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM captures.users WHERE workos_user_id = 'same'")
            .fetch_one(pool)
            .await?;
    assert_eq!(count, 1, "concurrent login must create one account");

    db::sync_authenticated(pool, &user("other", "same@example.com", now)).await?;
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM captures.users WHERE email = 'same@example.com'")
            .fetch_one(pool)
            .await?;
    assert_eq!(count, 2, "email equality must not merge identities");

    let mut tx = pool.begin().await?;
    db::event_update(
        &mut tx,
        &user("same", "new@example.com", now + ChronoDuration::minutes(2)),
    )
    .await?;
    db::event_update(
        &mut tx,
        &user(
            "same",
            "stale@example.com",
            now + ChronoDuration::minutes(1),
        ),
    )
    .await?;
    tx.commit().await?;
    let email: Option<String> =
        sqlx::query_scalar("SELECT email FROM captures.users WHERE workos_user_id = 'same'")
            .fetch_one(pool)
            .await?;
    assert_eq!(email.as_deref(), Some("new@example.com"));

    let mut tx = pool.begin().await?;
    db::event_delete(&mut tx, "deleted-before-login", now).await?;
    tx.commit().await?;
    assert!(
        db::sync_authenticated(
            pool,
            &user(
                "deleted-before-login",
                "late@example.com",
                now + ChronoDuration::hours(1)
            )
        )
        .await?
        .is_none()
    );

    sqlx::query("UPDATE captures.users SET disabled_at=now() WHERE workos_user_id='same'")
        .execute(pool)
        .await?;
    assert!(
        db::sync_authenticated(
            pool,
            &user(
                "same",
                "enabled-again@example.com",
                now + ChronoDuration::hours(2)
            )
        )
        .await?
        .is_none()
    );

    let mut tx = pool.begin().await?;
    assert!(db::record_event(&mut tx, "event-1", "user.updated").await?);
    assert!(!db::record_event(&mut tx, "event-1", "user.updated").await?);
    tx.commit().await?;
    let mut tx = pool.begin().await?;
    assert!(db::record_event(&mut tx, "rolled-back", "user.updated").await?);
    tx.rollback().await?;
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM captures.workos_events WHERE event_id='rolled-back')",
    )
    .fetch_one(pool)
    .await?;
    assert!(
        !exists,
        "failed webhook transactions must not retain receipts"
    );
    Ok(())
}

async fn http_regressions(pool: &PgPool) -> Result<(), sqlx::Error> {
    use axum::{body::Bytes, extract::Path, response::IntoResponse, routing::post};
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    let key = signing_key();
    let set = jwks(&key, "test");
    let base = mock_server(
        Router::new()
            .route("/sso/jwks/client", get(move || {
                let set = set.clone();
                async move { Json(set) }
            }))
            .route("/user_management/users/{id}", get(|Path(id): Path<String>| async move {
                match id.as_str() {
                    "user_deleted" => StatusCode::NOT_FOUND.into_response(),
                    "user_outage" => StatusCode::SERVICE_UNAVAILABLE.into_response(),
                    _ => Json(json!({"id":id,"email":"http@example.com","email_verified":true,"updated_at":Utc::now()})).into_response(),
                }
            })),
    ).await;
    let client = reqwest::Client::new();
    let state = AppState {
        verifier: JwtVerifier::new(client.clone(), &base, "client", "issuer"),
        config: Config {
            database_url: "unused".into(),
            api_key: "test".into(),
            client_id: "client".into(),
            webhook_secret: "test-secret".into(),
            issuer: "issuer".into(),
            bind: "127.0.0.1:0".parse().unwrap(),
            api_base: base,
        },
        pool: pool.clone(),
        client,
    };
    let app = Router::new()
        .route("/api/account/me", get(crate::me))
        .route("/api/webhooks/workos", post(crate::webhook::handle))
        .with_state(state);
    for (subject, status) in [
        ("user_http", StatusCode::OK),
        ("user_deleted", StatusCode::FORBIDDEN),
        ("user_outage", StatusCode::SERVICE_UNAVAILABLE),
    ] {
        let mut payload = claims("issuer", json!("client"));
        payload["sub"] = json!(subject);
        let response = app
            .clone()
            .oneshot(
                Request::get("/api/account/me")
                    .header(
                        header::AUTHORIZATION,
                        format!("Bearer {}", token(&key, "test", &payload)),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
    let deleted: bool = sqlx::query_scalar(
        "SELECT deleted_at IS NOT NULL FROM captures.users WHERE workos_user_id='user_deleted'",
    )
    .fetch_one(pool)
    .await?;
    assert!(deleted, "confirmed WorkOS 404 creates a tombstone");
    let outage: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM captures.users WHERE workos_user_id='user_outage'",
    )
    .fetch_one(pool)
    .await?;
    assert_eq!(
        outage, 0,
        "WorkOS outage must not delete/provision an account"
    );

    for (id, kind, data, expected) in [
        (
            "http-delete",
            "user.deleted",
            json!({"id":"user_http"}),
            StatusCode::OK,
        ),
        (
            "http-delete",
            "user.deleted",
            json!({"id":"user_http"}),
            StatusCode::OK,
        ),
        (
            "http-invalid",
            "user.updated",
            json!({"id":"user_http"}),
            StatusCode::SERVICE_UNAVAILABLE,
        ),
    ] {
        let body =
            Bytes::from(serde_json::to_vec(&json!({"id":id,"event":kind,"data":data})).unwrap());
        let timestamp = Utc::now().timestamp_millis();
        let mut mac = Hmac::<Sha256>::new_from_slice(b"test-secret").unwrap();
        mac.update(format!("{timestamp}.").as_bytes());
        mac.update(&body);
        let signature = format!(
            "t={timestamp},v1={}",
            hex::encode(mac.finalize().into_bytes())
        );
        let response = app
            .clone()
            .oneshot(
                Request::post("/api/webhooks/workos")
                    .header("WorkOS-Signature", signature)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM captures.workos_events WHERE event_id LIKE 'http-%'",
    )
    .fetch_one(pool)
    .await?;
    assert_eq!(
        count, 1,
        "duplicates and failed processing must not add receipts"
    );
    assert!(
        db::sync_authenticated(
            pool,
            &user("user_http", "resurrect@example.com", Utc::now())
        )
        .await?
        .is_none()
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL configured by TEST_DATABASE_URL"]
async fn postgres_account_and_webhook_regressions() {
    let admin_url = std::env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL must point to a disposable PostgreSQL server");
    let name = format!(
        "captures_test_{}_{}",
        std::process::id(),
        Utc::now().timestamp_micros().unsigned_abs()
    );
    let options = sqlx::postgres::PgConnectOptions::from_str(&admin_url).unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options.clone())
        .await
        .unwrap();
    sqlx::query(&format!("CREATE DATABASE {name}"))
        .execute(&admin)
        .await
        .unwrap();
    let test_options = options.database(&name);
    let test_url = test_options.to_url_lossy().to_string();
    let result = async {
        let (first, second) = tokio::join!(crate::migrate(&test_url), crate::migrate(&test_url));
        first.expect("first concurrent startup migration");
        second.expect("second concurrent startup migration");
        let pool = crate::connect(&test_url).await?;
        let search_path: String = sqlx::query_scalar("SHOW search_path")
            .fetch_one(&pool)
            .await?;
        assert!(
            !search_path.contains("captures"),
            "runtime must not depend on the migration search_path: {search_path}"
        );
        let public_tables: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM information_schema.tables WHERE table_schema='public'",
        )
        .fetch_one(&pool)
        .await?;
        assert_eq!(public_tables, 0, "migration must not contaminate public schema");
        let migration_schema: String = sqlx::query_scalar(
            "SELECT table_schema FROM information_schema.tables WHERE table_name='_sqlx_migrations'",
        )
        .fetch_one(&pool)
        .await?;
        assert_eq!(migration_schema, "captures");
        database_regressions(&pool).await?;
        http_regressions(&pool).await?;
        pool.close().await;
        Ok::<_, sqlx::Error>(())
    }
    .await;
    sqlx::query(&format!("DROP DATABASE {name}"))
        .execute(&admin)
        .await
        .expect("drop owned test database");
    result.expect("database regressions");
}
