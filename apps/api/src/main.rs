mod auth;
mod config;
mod db;
mod webhook;
mod workos;

#[cfg(test)]
mod tests;

use std::{str::FromStr, time::Duration};

use auth::{AuthError, JwtVerifier};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use config::Config;
use serde_json::json;
use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

#[derive(Clone)]
struct AppState {
    config: Config,
    pool: PgPool,
    client: reqwest::Client,
    verifier: JwtVerifier,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    if std::env::args().nth(1).as_deref() == Some("migrate") {
        let url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
            eprintln!("DATABASE_URL is required");
            std::process::exit(2)
        });
        let pool = connect_for_migration(&url).await.unwrap_or_else(|_| {
            eprintln!("database connection failed");
            std::process::exit(2)
        });
        MIGRATOR.run(&pool).await.unwrap_or_else(|_| {
            eprintln!("database migration failed");
            std::process::exit(2)
        });
        return;
    }
    if std::env::args().len() != 1 {
        eprintln!("usage: captures-api [migrate]");
        std::process::exit(2);
    }
    let config = Config::from_env().unwrap_or_else(|e| {
        eprintln!("configuration error: {e}");
        std::process::exit(2)
    });
    let pool = connect(&config.database_url).await.unwrap_or_else(|_| {
        eprintln!("database connection failed");
        std::process::exit(2)
    });
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .connect_timeout(Duration::from_secs(3))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_else(|_| {
            eprintln!("HTTP client setup failed");
            std::process::exit(2)
        });
    let verifier = JwtVerifier::new(
        client.clone(),
        &config.api_base,
        &config.client_id,
        &config.issuer,
    );
    let bind = config.bind;
    let state = AppState {
        config,
        pool,
        client,
        verifier,
    };
    let app = Router::new()
        .route("/health", get(|| async { Json(json!({"status":"ok"})) }))
        .route("/api/account/me", get(me))
        .route("/api/webhooks/workos", post(webhook::handle))
        .layer(DefaultBodyLimit::max(256 * 1024))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .unwrap_or_else(|_| {
            eprintln!("server bind failed");
            std::process::exit(2)
        });
    tracing::info!(%bind, "captures API listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown())
        .await
        .unwrap_or_else(|_| eprintln!("server stopped unexpectedly"));
}

async fn connect(url: &str) -> Result<PgPool, sqlx::Error> {
    let options = PgConnectOptions::from_str(url)?.options([("search_path", "captures")]);
    PgPoolOptions::new()
        .max_connections(10)
        .acquire_timeout(Duration::from_secs(5))
        .connect_with(options)
        .await
}

async fn connect_for_migration(url: &str) -> Result<PgPool, sqlx::Error> {
    let admin = PgPoolOptions::new().max_connections(1).connect(url).await?;
    sqlx::query("CREATE SCHEMA IF NOT EXISTS captures")
        .execute(&admin)
        .await?;
    admin.close().await;
    connect(url).await
}

async fn me(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(token) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|v| !v.is_empty())
    else {
        return error(StatusCode::UNAUTHORIZED);
    };
    let subject = match state.verifier.verify(token).await {
        Ok(v) => v,
        Err(AuthError::Invalid) => return error(StatusCode::UNAUTHORIZED),
        Err(AuthError::Unavailable) => return error(StatusCode::SERVICE_UNAVAILABLE),
    };
    let user = match workos::fetch_user(
        &state.client,
        &state.config.api_base,
        &state.config.api_key,
        &subject,
    )
    .await
    {
        Ok(v) => v,
        Err(workos::FetchError::NotFound) => {
            let Ok(mut tx) = state.pool.begin().await else {
                return error(StatusCode::SERVICE_UNAVAILABLE);
            };
            if db::event_delete(&mut tx, &subject, chrono::Utc::now())
                .await
                .is_err()
                || tx.commit().await.is_err()
            {
                return error(StatusCode::SERVICE_UNAVAILABLE);
            }
            return error(StatusCode::FORBIDDEN);
        }
        Err(workos::FetchError::Unavailable) => return error(StatusCode::SERVICE_UNAVAILABLE),
    };
    match db::sync_authenticated(&state.pool, &user).await {
        Ok(Some(account)) => protected((StatusCode::OK, Json(account)).into_response()),
        Ok(None) => error(StatusCode::FORBIDDEN),
        Err(_) => error(StatusCode::SERVICE_UNAVAILABLE),
    }
}

fn error(status: StatusCode) -> Response {
    protected(
        (
            status,
            Json(json!({"error": status.canonical_reason().unwrap_or("Request failed")})),
        )
            .into_response(),
    )
}
fn protected(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
async fn shutdown() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("install SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = terminate.recv() => {},
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}
