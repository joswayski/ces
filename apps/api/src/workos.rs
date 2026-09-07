use chrono::{DateTime, Utc};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct WorkosUser {
    pub id: String,
    pub email: String,
    #[serde(default)]
    pub email_verified: bool,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("user not found")]
    NotFound,
    #[error("identity service unavailable")]
    Unavailable,
}

pub async fn fetch_user(
    client: &reqwest::Client,
    base: &str,
    key: &str,
    subject: &str,
) -> Result<WorkosUser, FetchError> {
    let response = client
        .get(format!("{base}/user_management/users/{subject}"))
        .bearer_auth(key)
        .send()
        .await
        .map_err(|_| FetchError::Unavailable)?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(FetchError::NotFound);
    }
    let user: WorkosUser = response
        .error_for_status()
        .map_err(|_| FetchError::Unavailable)?
        .json()
        .await
        .map_err(|_| FetchError::Unavailable)?;
    if user.id != subject {
        return Err(FetchError::Unavailable);
    }
    Ok(user)
}
