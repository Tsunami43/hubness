//! HTTP helpers shared by the routers: JSON bodies, the CSRF guard.

use axum::body::Bytes;
use axum::extract::{FromRequest, Request};
use axum::http::{Method, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde::de::{DeserializeOwned, Deserializer};

use crate::error::{ApiResult, HttpError};

/// A JSON request body checked against `T`. Unlike `axum::Json` it does not require a
/// content type (the CSRF guard handles that) and any mismatch is a 400 `{error}` that
/// names the field, as the TypeScript server does.
pub struct JsonBody<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for JsonBody<T> {
    type Rejection = HttpError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let bytes = Bytes::from_request(req, state).await.map_err(|_| HttpError::bad_request("Invalid JSON"))?;
        let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| HttpError::bad_request("Invalid JSON"))?;
        if !value.is_object() {
            return Err(HttpError::bad_request("Request body must be a JSON object"));
        }
        serde_path_to_error::deserialize(value).map(JsonBody).map_err(|e| {
            let path = e.path().to_string();
            let inner = e.into_inner().to_string();
            HttpError::bad_request(if path == "." { inner } else { format!("{path}: {inner}") })
        })
    }
}

/// An optional JSON body (actions that take optional parameters): no body, a body that is
/// not JSON or broken JSON counts as `{}`; a JSON body that doesn't match `T` is still a 400.
pub struct LenientBody<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for LenientBody<T> {
    type Rejection = HttpError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let is_json = req.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|v| v.contains("json"));
        let bytes = Bytes::from_request(req, state).await.unwrap_or_default();
        let value = if is_json { serde_json::from_slice(&bytes).unwrap_or_else(|_| serde_json::json!({})) } else { serde_json::json!({}) };
        serde_path_to_error::deserialize(value).map(LenientBody).map_err(|e| {
            let path = e.path().to_string();
            let inner = e.into_inner().to_string();
            HttpError::bad_request(if path == "." { inner } else { format!("{path}: {inner}") })
        })
    }
}

/// Run blocking work of a handler (file system walks, child processes, docker calls) on
/// the blocking thread pool, so it never stalls the async threads that serve other
/// requests and streams.
pub async fn blocking<T: Send + 'static>(f: impl FnOnce() -> ApiResult<T> + Send + 'static) -> ApiResult<axum::Json<T>> {
    tokio::task::spawn_blocking(f).await.map_err(|e| HttpError::new(500, e.to_string()))?.map(axum::Json)
}

/// For `Option<Option<T>>` fields: absent → `None`, `null` → `Some(None)`, value → `Some(Some(v))`.
/// Use with `#[serde(default, deserialize_with = "double_option")]`.
pub fn double_option<'de, T: Deserialize<'de>, D: Deserializer<'de>>(d: D) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

/// CSRF protection: a mutating API request must be JSON or carry the `X-Hubness-Request`
/// header. A browser won't send such a request from a foreign origin without a CORS
/// preflight, and the hub doesn't allow preflight.
pub async fn csrf_guard(req: Request, next: Next) -> Response {
    let safe = matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    let is_json = req.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(is_json_type).unwrap_or(false);
    if !safe && !is_json && !req.headers().contains_key("x-hubness-request") {
        return HttpError::new(403, "Requests that change data must be JSON or carry X-Hubness-Request").into_response();
    }
    next.run(req).await
}

/// `application/json` followed by a word boundary (like the TS `/^application\/json\b/i`).
fn is_json_type(v: &str) -> bool {
    let v = v.to_ascii_lowercase();
    match v.strip_prefix("application/json") {
        Some(rest) => !rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_'),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::is_json_type;

    #[test]
    fn json_content_types() {
        assert!(is_json_type("application/json"));
        assert!(is_json_type("Application/JSON; charset=utf-8"));
        assert!(!is_json_type("application/jsonx"));
        assert!(!is_json_type("text/plain"));
    }
}
