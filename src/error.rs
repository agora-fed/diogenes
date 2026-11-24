//! The response envelope and the error type. The envelope has the same shape as the
//! AGORA envelope: `{ "success", "data", "error": { "code", "message" } }`.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

/// The JSON envelope of every API response.
#[derive(Debug, Serialize)]
pub struct Envelope<T: Serialize> {
    /// True when the call succeeded.
    pub success: bool,
    /// The payload when the call succeeded.
    pub data: Option<T>,
    /// The error when the call failed.
    pub error: Option<ErrorBody>,
}

/// The error part of the envelope.
#[derive(Debug, Serialize)]
pub struct ErrorBody {
    /// A stable machine code, for example `not_found`.
    pub code: &'static str,
    /// A message for people (pt-BR). It contains no sensitive data.
    pub message: String,
}

/// Wrap a payload in a success envelope.
pub fn ok<T: Serialize>(data: T) -> Json<Envelope<T>> {
    Json(Envelope {
        success: true,
        data: Some(data),
        error: None,
    })
}

/// The error of a DIOGENES handler.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// Nobody is signed in.
    #[error("unauthenticated")]
    Unauthenticated,
    /// The caller has no access to the resource.
    #[error("forbidden")]
    Forbidden,
    /// The resource does not exist, or the caller cannot see it.
    #[error("not found")]
    NotFound,
    /// The module is off for the organization of the caller.
    #[error("module disabled")]
    ModuleDisabled,
    /// The user has too many open live-update streams.
    #[error("too many streams")]
    TooManyStreams,
    /// The request is not valid.
    #[error("invalid: {0}")]
    Invalid(String),
    /// A database error.
    #[error("database: {0}")]
    Db(#[from] sqlx::Error),
}

/// The result type of DIOGENES handlers.
pub type ApiResult<T> = Result<T, ApiError>;

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = match &self {
            Self::Unauthenticated => (
                StatusCode::UNAUTHORIZED,
                "unauthenticated",
                "Entre na sua conta para continuar.".to_owned(),
            ),
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                "forbidden",
                "Você não tem permissão para esta ação.".to_owned(),
            ),
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "not_found",
                "Não encontrado.".to_owned(),
            ),
            Self::ModuleDisabled => (
                StatusCode::NOT_FOUND,
                "module_disabled",
                "Este recurso não está disponível nesta organização.".to_owned(),
            ),
            Self::TooManyStreams => (
                StatusCode::TOO_MANY_REQUESTS,
                "too_many_streams",
                "Muitas abas abertas nesta sala. Feche algumas e tente de novo.".to_owned(),
            ),
            Self::Invalid(msg) => (StatusCode::BAD_REQUEST, "invalid", msg.clone()),
            Self::Db(err) => {
                tracing::error!(error = %err, "diogenes database error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "Erro interno.".to_owned(),
                )
            }
        };
        let body: Envelope<()> = Envelope {
            success: false,
            data: None,
            error: Some(ErrorBody { code, message }),
        };
        (status, Json(body)).into_response()
    }
}
