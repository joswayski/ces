use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use axum::{
    Json,
    body::Bytes,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use tokio::sync::Mutex;

const MANIFEST_URL: &str =
    "https://github.com/joswayski/captures/releases/download/preview/latest.json";
const MAX_BODY_BYTES: usize = 32 * 1024;

#[derive(Clone)]
pub struct ApiState {
    client: reqwest::Client,
    webhook: Option<String>,
    manifest: Arc<Mutex<Option<CachedManifest>>>,
    feedback_clients: Arc<Mutex<HashMap<String, Instant>>>,
}

struct CachedManifest {
    text: String,
    fetched: SystemTime,
    expires: Instant,
}

impl ApiState {
    pub fn new(webhook: Option<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            webhook,
            manifest: Arc::new(Mutex::new(None)),
            feedback_clients: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

pub async fn preview(State(state): State<ApiState>) -> Response {
    if let Some(response) = cached_manifest(&state, false).await {
        return response;
    }
    let fetched = state
        .client
        .get(MANIFEST_URL)
        .header(header::ACCEPT, "application/json")
        .header(header::USER_AGENT, "captures-api")
        .timeout(Duration::from_secs(5))
        .send()
        .await;
    if let Ok(response) = fetched
        && response.status().is_success()
        && let Ok(bytes) = response.bytes().await
        && bytes.len() <= 256 * 1024
        && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
        && value
            .get("version")
            .and_then(Value::as_str)
            .is_some_and(|v| !v.trim().is_empty())
        && value.get("platforms").is_some_and(Value::is_object)
    {
        let text = String::from_utf8_lossy(&bytes).into_owned();
        *state.manifest.lock().await = Some(CachedManifest {
            text: text.clone(),
            fetched: SystemTime::now(),
            expires: Instant::now() + Duration::from_secs(60),
        });
        return manifest_response(text, 0);
    }
    cached_manifest(&state, true).await.unwrap_or_else(|| {
        api_json(
            StatusCode::BAD_GATEWAY,
            json!({"error":"updater manifest is unavailable"}),
            None,
        )
    })
}

async fn cached_manifest(state: &ApiState, stale: bool) -> Option<Response> {
    let cache = state.manifest.lock().await;
    let value = cache.as_ref()?;
    if !stale && Instant::now() >= value.expires {
        return None;
    }
    let age = SystemTime::now()
        .duration_since(value.fetched)
        .unwrap_or_default()
        .as_secs();
    Some(manifest_response(value.text.clone(), age))
}

fn manifest_response(text: String, age: u64) -> Response {
    let mut response = (StatusCode::OK, text).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=60"),
    );
    response.headers_mut().insert(
        header::AGE,
        HeaderValue::from_str(&age.to_string()).expect("numeric age"),
    );
    response
}

pub async fn feedback(State(state): State<ApiState>, headers: HeaderMap, body: Bytes) -> Response {
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    if origin.is_some_and(|v| !allowed_origin(v)) {
        return api_json(
            StatusCode::FORBIDDEN,
            json!({"error":"origin not allowed"}),
            origin,
        );
    }
    if body.len() > MAX_BODY_BYTES {
        return api_json(
            StatusCode::PAYLOAD_TOO_LARGE,
            json!({"error":"request body is too large"}),
            origin,
        );
    }
    let value: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => {
            return api_json(
                StatusCode::BAD_REQUEST,
                json!({"error":"request body must be valid JSON"}),
                origin,
            );
        }
    };
    let feedback = match parse_feedback(&value) {
        Ok(v) => v,
        Err(error) => return api_json(StatusCode::BAD_REQUEST, json!({"error":error}), origin),
    };
    let Some(webhook) = &state.webhook else {
        return api_json(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"error":"feedback service is not configured"}),
            origin,
        );
    };
    let client = headers
        .get("cf-connecting-ip")
        .or_else(|| headers.get("x-real-ip"))
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or("unknown")
        .to_owned();
    let mut limits = state.feedback_clients.lock().await;
    limits.retain(|_, time| time.elapsed() < Duration::from_secs(60));
    if limits.contains_key(&client) {
        return api_json(
            StatusCode::TOO_MANY_REQUESTS,
            json!({"error":"please wait a minute before sending more feedback"}),
            origin,
        );
    }
    limits.insert(client.clone(), Instant::now());
    drop(limits);
    let result = state
        .client
        .post(webhook)
        .timeout(Duration::from_secs(10))
        .json(&discord_payload(&feedback, &client))
        .send()
        .await;
    if !result.is_ok_and(|r| r.status().is_success()) {
        state.feedback_clients.lock().await.remove(&client);
        return api_json(
            StatusCode::BAD_GATEWAY,
            json!({"error":"failed to deliver feedback"}),
            origin,
        );
    }
    api_json(StatusCode::CREATED, json!({"ok":true}), origin)
}

pub async fn feedback_options(headers: HeaderMap) -> Response {
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    if origin.is_some_and(|v| !allowed_origin(v)) {
        return api_json(
            StatusCode::FORBIDDEN,
            json!({"error":"origin not allowed"}),
            origin,
        );
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    add_cors(response.headers_mut(), origin);
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("Content-Type"),
    );
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("POST, OPTIONS"),
    );
    response
}

struct Feedback {
    message: String,
    contact: Option<String>,
    category: String,
    source: String,
    app: Option<String>,
    os: Option<String>,
    os_version: Option<String>,
    arch: Option<String>,
}
fn parse_feedback(value: &Value) -> Result<Feedback, String> {
    let object = value
        .as_object()
        .ok_or("request body must be a JSON object")?;
    let required = |name, max| string(object.get(name), name, max, true);
    let optional = |name, max| string(object.get(name), name, max, false);
    let message = required("message", 8000)?.expect("required");
    let category = optional("category", 128)?
        .unwrap_or_else(|| "bug".into())
        .to_lowercase();
    if !["bug", "idea", "other", "crash"].contains(&category.as_str()) {
        return Err("category must be one of: bug, idea, other, crash".into());
    }
    let source = optional("source", 128)?
        .unwrap_or_else(|| "desktop".into())
        .to_lowercase();
    if !["desktop", "web"].contains(&source.as_str()) {
        return Err("source must be one of: desktop, web".into());
    }
    Ok(Feedback {
        message,
        contact: optional("contact", 200)?,
        category,
        source,
        app: optional("app_version", 128)?,
        os: optional("os", 128)?,
        os_version: optional("os_version", 128)?,
        arch: optional("arch", 128)?,
    })
}
fn string(
    value: Option<&Value>,
    name: &str,
    max: usize,
    required: bool,
) -> Result<Option<String>, String> {
    if value.is_none()
        || value == Some(&Value::Null)
        || value == Some(&Value::String(String::new()))
    {
        return if required {
            Err(format!("{name} is required"))
        } else {
            Ok(None)
        };
    }
    let text = value
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{name} must be a string"))?
        .trim()
        .to_owned();
    if text.is_empty() {
        return if required {
            Err(format!("{name} is required"))
        } else {
            Ok(None)
        };
    }
    if text.chars().count() > max {
        return Err(format!("{name} must be at most {max} characters"));
    }
    Ok(Some(text))
}
fn discord_payload(f: &Feedback, client: &str) -> Value {
    let (title, color) = match f.category.as_str() {
        "idea" => ("Idea", 0x58a6ff),
        "other" => ("Other feedback", 0x8b949e),
        "crash" => ("Crash report", 0xd29922),
        _ => ("Bug report", 0xef4650),
    };
    let mut fields = vec![
        json!({"name":"Category","value":f.category,"inline":true}),
        json!({"name":"Source","value":f.source,"inline":true}),
    ];
    if let Some(v) = &f.app {
        fields.push(json!({"name":"App","value":v,"inline":true}));
    }
    let system = [f.os.as_deref(), f.os_version.as_deref(), f.arch.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
    if !system.is_empty() {
        fields.push(json!({"name":"System","value":system,"inline":true}));
    }
    if let Some(v) = &f.contact {
        fields.push(json!({"name":"Contact","value":v,"inline":true}));
    }
    fields.push(json!({"name":"Client","value":truncate(client,64),"inline":true}));
    json!({"embeds":[{"title":title,"description":description(f),"color":color,"fields":fields}]})
}
fn description(f: &Feedback) -> String {
    if f.category != "crash" {
        return truncate(&f.message, 4000);
    }
    let Some((intro, detail)) = f.message.split_once("\n\n") else {
        return truncate(&f.message, 4000);
    };
    let detail = detail.trim().replace("```", "'''");
    let intro = truncate(intro.trim(), 3991);
    let budget = 4000usize.saturating_sub(intro.chars().count() + 9);
    if budget == 0 {
        return intro;
    }
    format!("{intro}\n\n```\n{}\n```", truncate(&detail, budget))
}
fn truncate(v: &str, max: usize) -> String {
    if v.chars().count() <= max {
        return v.into();
    }
    v.chars()
        .take(max.saturating_sub(1))
        .chain(std::iter::once('…'))
        .collect()
}
fn allowed_origin(v: &str) -> bool {
    matches!(
        v,
        "https://captur.es" | "http://localhost:5174" | "http://127.0.0.1:5174"
    )
}
fn api_json(status: StatusCode, value: Value, origin: Option<&str>) -> Response {
    let mut r = (status, Json(value)).into_response();
    r.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    add_cors(r.headers_mut(), origin);
    r
}
fn add_cors(headers: &mut HeaderMap, origin: Option<&str>) {
    if let Some(v) = origin.filter(|v| allowed_origin(v)) {
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_str(v).expect("allowed origin"),
        );
        headers.insert(header::VARY, HeaderValue::from_static("Origin"));
    }
}
