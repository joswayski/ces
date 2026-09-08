use std::{env, net::SocketAddr};

#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub bind: SocketAddr,
    pub discord_webhook_url: Option<String>,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let bind = env::var("CAPTURES_API_BIND")
            .unwrap_or_else(|_| "127.0.0.1:3001".into())
            .parse()
            .map_err(|_| "CAPTURES_API_BIND is invalid".to_string())?;
        Ok(Self {
            database_url: database_url("DATABASE_URL", false)?,
            bind,
            discord_webhook_url: discord_webhook_url()?,
        })
    }
}

fn discord_webhook_url() -> Result<Option<String>, String> {
    let Some(value) = env::var("DISCORD_WEBHOOK_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    let url = reqwest::Url::parse(&value)
        .map_err(|_| "DISCORD_WEBHOOK_URL must be a valid Discord HTTPS webhook URL".to_string())?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("discord.com" | "discordapp.com"))
        || !url.path().starts_with("/api/webhooks/")
    {
        return Err("DISCORD_WEBHOOK_URL must be a valid Discord HTTPS webhook URL".into());
    }
    Ok(Some(value))
}

fn required(name: &str) -> Result<String, String> {
    env::var(name)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| format!("{name} is required"))
}

pub fn database_url(name: &str, migration: bool) -> Result<String, String> {
    let value = required(name)?;
    validate_database_url(&value, migration).map_err(|reason| format!("{name}: {reason}"))?;
    Ok(value)
}

/// Check the original URL: SQLx's parsed options contain credentials and must
/// never be included in errors. Loopback-only development may use plaintext connections.
fn validate_database_url(value: &str, migration: bool) -> Result<reqwest::Url, &'static str> {
    let url = reqwest::Url::parse(value).map_err(|_| "invalid PostgreSQL URL")?;
    if !matches!(url.scheme(), "postgres" | "postgresql") || url.host_str().is_none() {
        return Err("expected a PostgreSQL URL with a host");
    }
    let database = url.path().strip_prefix('/').unwrap_or_default();
    if database.is_empty()
        || !database
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    {
        return Err(
            "an explicit named database is required (letters, digits, underscores or hyphens)",
        );
    }
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if !local && matches!(database, "postgres" | "template0" | "template1") {
        return Err("use the dedicated captures database, not a shared/default database");
    }
    if migration && (url.port() == Some(6432) || (!local && url.port().unwrap_or(5432) != 5432)) {
        return Err(
            "migrations require the direct PostgreSQL endpoint (5432), not PgBouncer (6432)",
        );
    }
    let mut sslmode = None;
    let mut sslrootcert = None;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "sslmode" if sslmode.is_none() => sslmode = Some(value.into_owned()),
            "sslrootcert" if sslrootcert.is_none() => sslrootcert = Some(value.into_owned()),
            "application_name" => {}
            // Reject aliases/overrides that could disagree with the checked host,
            // port, database or TLS settings rather than silently accepting them.
            _ => return Err("unsupported or duplicate connection URL parameter"),
        }
    }
    if !local && sslmode.as_deref() != Some("verify-full") {
        return Err("remote database connections require sslmode=verify-full");
    }
    if sslrootcert
        .as_deref()
        .is_some_and(|cert| cert.is_empty() || cert == "system")
    {
        return Err(
            "SQLx requires a CA PEM file for sslrootcert; omit it to use bundled public roots",
        );
    }
    Ok(url)
}

pub fn validate_database_pair(runtime: &str, migration: &str) -> Result<(), String> {
    let runtime =
        validate_database_url(runtime, false).map_err(|e| format!("DATABASE_URL: {e}"))?;
    let migration = validate_database_url(migration, true)
        .map_err(|e| format!("MIGRATION_DATABASE_URL: {e}"))?;
    if runtime.host_str() != migration.host_str() || runtime.path() != migration.path() {
        return Err(
            "DATABASE_URL and MIGRATION_DATABASE_URL must target the same host and named database"
                .into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_roles_share_a_database_but_not_a_pooler_endpoint() {
        let runtime = "postgres://app:password@db.example:6432/captures?sslmode=verify-full";
        let migration = "postgres://owner:password@db.example:5432/captures?sslmode=verify-full";
        assert!(validate_database_pair(runtime, migration).is_ok());
        assert!(validate_database_pair(runtime, runtime).is_err());
        assert!(
            validate_database_pair(runtime, &migration.replace("/captures?", "/caperchat?"))
                .is_err()
        );
        assert!(
            validate_database_pair(runtime, &migration.replace("db.example", "other.example"))
                .is_err()
        );
    }

    #[test]
    fn unsafe_or_ambiguous_remote_urls_are_rejected_without_secrets() {
        for value in [
            "not-a-url",
            "https://app:password@db.example/captures?sslmode=verify-full",
            "postgres://app:password@db.example/?sslmode=verify-full",
            "postgres://app:password@db.example/postgres?sslmode=verify-full",
            "postgres://app:password@db.example/captures?sslmode=require",
            "postgres://app:password@db.example/captures?sslmode=verify-full&sslmode=disable",
            "postgres://app:password@db.example/captures?sslmode=verify-full&sslrootcert=system",
            "postgres://app:password@localhost/captures?host=db.example",
        ] {
            let error = validate_database_url(value, true).unwrap_err();
            assert!(!error.contains("password"));
        }
        assert!(validate_database_url("postgres://app:password@db.example/captures?sslmode=verify-full&sslrootcert=/etc/ssl/certs/ca-certificates.crt", true).is_ok());
        assert!(
            validate_database_url("postgres://test@127.0.0.1:55432/captures_test", true).is_ok()
        );
        assert!(
            validate_database_url("postgres://test@127.0.0.1:6432/captures_test", true).is_err()
        );
    }
}
