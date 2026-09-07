use std::{env, net::SocketAddr};

#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub api_key: String,
    pub client_id: String,
    pub webhook_secret: String,
    pub issuer: String,
    pub bind: SocketAddr,
    pub api_base: String,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        fn required(name: &str) -> Result<String, String> {
            env::var(name)
                .ok()
                .filter(|v| !v.is_empty())
                .ok_or_else(|| format!("{name} is required"))
        }
        let issuer = env::var("WORKOS_ISSUER").unwrap_or_else(|_| "https://api.workos.com/".into());
        let bind = env::var("CAPTURES_API_BIND")
            .unwrap_or_else(|_| "127.0.0.1:3001".into())
            .parse()
            .map_err(|_| "CAPTURES_API_BIND is invalid".to_string())?;
        Ok(Self {
            database_url: required("DATABASE_URL")?,
            api_key: required("WORKOS_API_KEY")?,
            client_id: required("WORKOS_CLIENT_ID")?,
            webhook_secret: required("WORKOS_WEBHOOK_SECRET")?,
            issuer,
            bind,
            api_base: "https://api.workos.com".into(),
        })
    }
}
