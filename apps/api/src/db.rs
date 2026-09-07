use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{PgPool, Postgres, Transaction};

use crate::workos::WorkosUser;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub email: String,
    pub email_verified: bool,
}

pub async fn sync_authenticated(
    pool: &PgPool,
    user: &WorkosUser,
) -> Result<Option<Account>, sqlx::Error> {
    let row = sqlx::query_as::<_, (Option<String>, bool, Option<DateTime<Utc>>, Option<DateTime<Utc>>)>(r#"
        INSERT INTO captures.users (workos_user_id, email, email_verified, workos_updated_at)
        VALUES ($1,$2,$3,$4)
        ON CONFLICT (workos_user_id) DO UPDATE SET
          email = CASE WHEN captures.users.deleted_at IS NULL AND (captures.users.workos_updated_at IS NULL OR captures.users.workos_updated_at <= EXCLUDED.workos_updated_at) THEN EXCLUDED.email ELSE captures.users.email END,
          email_verified = CASE WHEN captures.users.deleted_at IS NULL AND (captures.users.workos_updated_at IS NULL OR captures.users.workos_updated_at <= EXCLUDED.workos_updated_at) THEN EXCLUDED.email_verified ELSE captures.users.email_verified END,
          workos_updated_at = CASE WHEN captures.users.deleted_at IS NULL THEN GREATEST(captures.users.workos_updated_at, EXCLUDED.workos_updated_at) ELSE captures.users.workos_updated_at END,
          updated_at = CASE WHEN captures.users.deleted_at IS NULL THEN now() ELSE captures.users.updated_at END
        RETURNING email,email_verified,disabled_at,deleted_at"#)
        .bind(&user.id).bind(&user.email).bind(user.email_verified).bind(user.updated_at)
        .fetch_one(pool).await?;
    if row.2.is_some() || row.3.is_some() {
        return Ok(None);
    }
    Ok(row.0.map(|email| Account {
        email,
        email_verified: row.1,
    }))
}

pub async fn record_event(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
    kind: &str,
) -> Result<bool, sqlx::Error> {
    Ok(sqlx::query("INSERT INTO captures.workos_events(event_id,event_type) VALUES($1,$2) ON CONFLICT DO NOTHING")
        .bind(id).bind(kind).execute(&mut **tx).await?.rows_affected() == 1)
}

pub async fn event_update(
    tx: &mut Transaction<'_, Postgres>,
    user: &WorkosUser,
) -> Result<(), sqlx::Error> {
    sqlx::query(r#"UPDATE captures.users SET email=$2,email_verified=$3,workos_updated_at=$4,updated_at=now()
      WHERE workos_user_id=$1 AND deleted_at IS NULL AND (workos_updated_at IS NULL OR workos_updated_at < $4)"#)
      .bind(&user.id).bind(&user.email).bind(user.email_verified).bind(user.updated_at).execute(&mut **tx).await?;
    Ok(())
}

pub async fn event_delete(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
    at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query(r#"INSERT INTO captures.users(workos_user_id,email,deleted_at,workos_updated_at) VALUES($1,NULL,$2,$2)
      ON CONFLICT(workos_user_id) DO UPDATE SET email=NULL,email_verified=false,deleted_at=COALESCE(captures.users.deleted_at,$2),
      workos_updated_at=GREATEST(captures.users.workos_updated_at,$2),updated_at=now()"#)
      .bind(id).bind(at).execute(&mut **tx).await?;
    Ok(())
}
