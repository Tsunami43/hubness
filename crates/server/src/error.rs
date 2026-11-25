use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use hubness_core::ApiError;

/// An API error: rendered as `{"error": message}` with the status code. Client errors
/// carry a message meant for the user; anything else becomes a 500.
#[derive(Debug)]
pub struct HttpError {
    pub status: StatusCode,
    pub message: String,
}

pub type ApiResult<T> = Result<T, HttpError>;

impl HttpError {
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        Self { status: StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR), message: message.into() }
    }
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(400, message)
    }
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(404, message)
    }
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        if self.status.is_server_error() {
            eprintln!("hubness: {} {}", self.status.as_u16(), self.message);
        }
        (self.status, Json(ApiError { error: self.message })).into_response()
    }
}

impl From<rusqlite::Error> for HttpError {
    fn from(e: rusqlite::Error) -> Self {
        Self::new(500, format!("Database error: {e}"))
    }
}

impl From<std::io::Error> for HttpError {
    fn from(e: std::io::Error) -> Self {
        Self::new(500, e.to_string())
    }
}
