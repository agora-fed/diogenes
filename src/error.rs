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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    async fn render(err: ApiError) -> (StatusCode, serde_json::Value) {
        let response = err.into_response();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn too_many_streams_is_http_429_with_its_code() {
        let (status, body) = render(ApiError::TooManyStreams).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(body["success"], false);
        assert_eq!(body["data"], serde_json::Value::Null);
        assert_eq!(body["error"]["code"], "too_many_streams");
    }

    #[tokio::test]
    async fn database_error_is_http_500_and_hides_the_cause() {
        let (status, body) = render(ApiError::Db(sqlx::Error::Protocol(
            "secret relation diogenes_x".into(),
        )))
        .await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body["error"]["code"], "internal");
        assert_eq!(body["error"]["message"], "Erro interno.");
        assert!(!body.to_string().contains("secret"));
    }

    #[tokio::test]
    async fn each_error_has_a_status_and_a_stable_code() {
        let cases = [
            (
                ApiError::Unauthenticated,
                StatusCode::UNAUTHORIZED,
                "unauthenticated",
            ),
            (ApiError::Forbidden, StatusCode::FORBIDDEN, "forbidden"),
            (ApiError::NotFound, StatusCode::NOT_FOUND, "not_found"),
            (
                ApiError::ModuleDisabled,
                StatusCode::NOT_FOUND,
                "module_disabled",
            ),
            (
                ApiError::Invalid("x".into()),
                StatusCode::BAD_REQUEST,
                "invalid",
            ),
        ];
        for (err, expected_status, code) in cases {
            let (status, body) = render(err).await;
            assert_eq!(status, expected_status);
            assert_eq!(body["error"]["code"], code);
        }
    }

    #[test]
    fn ok_wraps_the_payload_in_a_success_envelope() {
        let Json(envelope) = ok(5);
        assert!(envelope.success);
        assert_eq!(envelope.data, Some(5));
        assert!(envelope.error.is_none());
    }
}
