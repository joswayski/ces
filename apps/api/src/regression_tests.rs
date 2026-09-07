use std::str::FromStr;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use chrono::Utc;
use sqlx::{ConnectOptions, postgres::PgPoolOptions};
use tower::ServiceExt;

#[tokio::test]
async fn account_requests_fail_closed_even_with_credentials() {
    for token in [None, Some("Bearer obsolete-token")] {
        let mut request = Request::get("/api/account/me");
        if let Some(token) = token {
            request = request.header(header::AUTHORIZATION, token);
        }
        let response = crate::router()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert!(!response.headers().contains_key(header::SET_COOKIE));
    }
}

#[tokio::test]
async fn health_remains_available() {
    let response = crate::router()
        .oneshot(Request::get("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL configured by TEST_DATABASE_URL"]
async fn postgres_users_and_startup_regressions() {
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
    let test_url = options.database(&name).to_url_lossy().to_string();
    let result = async {
        let (first, second) = tokio::join!(crate::migrate(&test_url), crate::migrate(&test_url));
        first.expect("first concurrent startup migration");
        second.expect("second concurrent startup migration");
        let pool = crate::connect(&test_url).await?;
        let search_path: String = sqlx::query_scalar("SHOW search_path")
            .fetch_one(&pool)
            .await?;
        assert!(!search_path.contains("captures"));
        let tables: Vec<(String, String)> = sqlx::query_as(
            "SELECT table_schema, table_name FROM information_schema.tables
             WHERE table_schema IN ('public', 'captures') ORDER BY table_schema, table_name",
        )
        .fetch_all(&pool)
        .await?;
        assert_eq!(
            tables,
            vec![
                ("captures".into(), "_sqlx_migrations".into()),
                ("captures".into(), "users".into()),
            ]
        );
        let columns: Vec<String> = sqlx::query_scalar(
            "SELECT column_name FROM information_schema.columns
             WHERE table_schema='captures' AND table_name='users' ORDER BY ordinal_position",
        )
        .fetch_all(&pool)
        .await?;
        assert_eq!(
            columns,
            vec![
                "id",
                "email",
                "email_verified",
                "created_at",
                "updated_at",
                "disabled_at",
                "deleted_at"
            ]
        );
        let (id, email, verified): (i64, Option<String>, bool) = sqlx::query_as(
            "INSERT INTO captures.users DEFAULT VALUES RETURNING id, email, email_verified",
        )
        .fetch_one(&pool)
        .await?;
        assert!(id > 0);
        assert_eq!(email, None);
        assert!(!verified, "new users must not be implicitly verified");
        sqlx::query("UPDATE captures.users SET email='test@example.com' WHERE id=$1")
            .bind(id)
            .execute(&pool)
            .await?;
        assert!(
            sqlx::query("UPDATE captures.users SET deleted_at=now() WHERE id=$1")
                .bind(id)
                .execute(&pool)
                .await
                .is_err()
        );
        sqlx::query("UPDATE captures.users SET email=NULL, deleted_at=now() WHERE id=$1")
            .bind(id)
            .execute(&pool)
            .await?;
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
