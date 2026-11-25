//! The client of the Câmara dos Deputados open-data API (`dadosabertos.camara.leg.br/api/v2`).
//!
//! The API sends HTTP 429 when a client sends requests too fast. The client:
//! * waits a minimum interval between two requests (one shared clock);
//! * retries 429 and 5xx with an increasing wait, and obeys `Retry-After`;
//! * follows the `next` link for paged lists.

pub mod dto;

use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::Deserialize;
use tokio::sync::Mutex;

/// The base URL of the API.
pub const BASE_URL: &str = "https://dadosabertos.camara.leg.br/api/v2";
/// The current legislature.
pub const LEGISLATURE: i32 = 57;
/// The Câmara id of the plenary ("PLEN").
pub const PLENARY_ID: i32 = 180;
/// The Câmara id of the board ("Mesa Diretora").
pub const BOARD_ID: i32 = 4;

const MIN_INTERVAL: Duration = Duration::from_millis(350);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_ATTEMPTS: u32 = 6;
const BASE_BACKOFF: Duration = Duration::from_secs(3);
const MAX_PAGES: usize = 200;
const PAGE_SIZE: u32 = 100;
const USER_AGENT: &str = concat!(
    "diogenes/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/agora-fed/diogenes)"
);

/// An error of the client.
#[derive(Debug, thiserror::Error)]
pub enum CamaraError {
    /// A transport error, or a body that is not valid JSON.
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    /// The API sent an error status after all retries.
    #[error("status {status} for {url}")]
    Status {
        /// The HTTP status.
        status: u16,
        /// The URL of the request.
        url: String,
    },
}

#[derive(Debug, Deserialize)]
struct Page<T> {
    dados: Vec<T>,
    #[serde(default)]
    links: Vec<Link>,
}

#[derive(Debug, Deserialize)]
struct Single<T> {
    dados: T,
}

#[derive(Debug, Deserialize)]
struct Link {
    rel: String,
    href: String,
}

/// The API client. Clone it freely: all clones share one clock.
#[derive(Debug, Clone)]
pub struct CamaraClient {
    http: reqwest::Client,
    base: String,
    last: std::sync::Arc<Mutex<Option<Instant>>>,
    min_interval: Duration,
    backoff: Duration,
}

impl CamaraClient {
    /// Make a client for the public API.
    ///
    /// # Errors
    /// Returns an error when the HTTP client cannot start (TLS setup).
    pub fn new() -> Result<Self, CamaraError> {
        Self::with_base(BASE_URL)
    }

    /// Make a client for another base URL (tests use a local server).
    ///
    /// # Errors
    /// Returns an error when the HTTP client cannot start (TLS setup).
    pub fn with_base(base: &str) -> Result<Self, CamaraError> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(REQUEST_TIMEOUT)
            .build()?;
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_owned(),
            last: std::sync::Arc::default(),
            min_interval: MIN_INTERVAL,
            backoff: BASE_BACKOFF,
        })
    }

    async fn pace(&self) {
        let mut last = self.last.lock().await;
        if let Some(at) = *last {
            let elapsed = at.elapsed();
            if elapsed < self.min_interval {
                tokio::time::sleep(self.min_interval - elapsed).await;
            }
        }
        *last = Some(Instant::now());
    }

    async fn get_url<T: DeserializeOwned>(&self, url: &str) -> Result<T, CamaraError> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            self.pace().await;
            let response = self
                .http
                .get(url)
                .header("Accept", "application/json")
                .send()
                .await;
            let response = match response {
                Ok(r) => r,
                Err(err) if attempt < MAX_ATTEMPTS && (err.is_timeout() || err.is_connect()) => {
                    tokio::time::sleep(self.backoff * attempt).await;
                    continue;
                }
                Err(err) => return Err(err.into()),
            };
            let status = response.status();
            if status.is_success() {
                return Ok(response.json::<T>().await?);
            }
            let retry = status.as_u16() == 429 || status.is_server_error();
            if !retry || attempt >= MAX_ATTEMPTS {
                return Err(CamaraError::Status {
                    status: status.as_u16(),
                    url: url.to_owned(),
                });
            }
            let wait = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .map_or(self.backoff * attempt, Duration::from_secs);
            tracing::debug!(%url, status = status.as_u16(), ?wait, "camara api retry");
            tokio::time::sleep(wait).await;
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base, path.trim_start_matches('/'))
    }

    /// Get one object (the `dados` field of the body).
    ///
    /// # Errors
    /// Returns the transport or status error after all retries.
    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, CamaraError> {
        let single: Single<T> = self.get_url(&self.url(path)).await?;
        Ok(single.dados)
    }

    /// Get all pages of a list. The path can hold query parameters.
    ///
    /// # Errors
    /// Returns the transport or status error after all retries.
    pub async fn list<T: DeserializeOwned>(&self, path: &str) -> Result<Vec<T>, CamaraError> {
        let sep = if path.contains('?') { '&' } else { '?' };
        let mut url = Some(self.url(&format!("{path}{sep}itens={PAGE_SIZE}")));
        let mut out = Vec::new();
        let mut pages = 0;
        while let Some(current) = url.take() {
            let page: Page<T> = self.get_url(&current).await?;
            out.extend(page.dados);
            pages += 1;
            if pages >= MAX_PAGES {
                tracing::warn!(%path, "camara api page limit reached");
                break;
            }
            url = page
                .links
                .into_iter()
                .find(|l| l.rel == "next")
                .map(|l| self.rebase(&l.href));
        }
        Ok(out)
    }

    /// The API returns absolute `next` links. Keep them on the configured base.
    fn rebase(&self, href: &str) -> String {
        href.strip_prefix(BASE_URL)
            .map_or_else(|| href.to_owned(), |rest| format!("{}{rest}", self.base))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use axum::extract::{Request, State};
    use axum::http::StatusCode;
    use axum::response::{IntoResponse, Response};
    use serde_json::{json, Value};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    type Handler = Arc<dyn Fn(usize, &str) -> Response + Send + Sync>;

    #[derive(Clone)]
    struct Fake {
        calls: Arc<AtomicUsize>,
        handler: Handler,
    }

    async fn respond(State(fake): State<Fake>, req: Request) -> Response {
        let n = fake.calls.fetch_add(1, Ordering::SeqCst) + 1;
        let target = req
            .uri()
            .path_and_query()
            .map(ToString::to_string)
            .unwrap_or_default();
        (fake.handler)(n, &target)
    }

    /// Start a local server on `[::1]`. The handler gets the call number (from 1) and the target.
    async fn serve(
        handler: impl Fn(usize, &str) -> Response + Send + Sync + 'static,
    ) -> (String, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let fake = Fake {
            calls: calls.clone(),
            handler: Arc::new(handler),
        };
        let app = axum::Router::new().fallback(respond).with_state(fake);
        let listener = tokio::net::TcpListener::bind("[::1]:0").await.unwrap();
        let base = format!("http://{}/api/v2", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (base, calls)
    }

    /// A client with no pacing and a short back-off, so the tests stay fast.
    fn quick_client(base: &str) -> CamaraClient {
        let mut client = CamaraClient::with_base(base).unwrap();
        client.min_interval = Duration::ZERO;
        client.backoff = Duration::from_millis(1);
        client
    }

    fn json_response(body: &Value) -> Response {
        axum::Json(body.clone()).into_response()
    }

    #[test]
    fn new_client_uses_the_public_base_url() {
        let client = CamaraClient::new().unwrap();
        assert_eq!(client.base, BASE_URL);
    }

    #[test]
    fn with_base_trims_the_trailing_slash() {
        let client = CamaraClient::with_base("http://[::1]:1/api/v2///").unwrap();
        assert_eq!(client.url("/deputados"), "http://[::1]:1/api/v2/deputados");
    }

    #[tokio::test]
    async fn get_returns_the_dados_field() {
        let (base, _) = serve(|_, _| json_response(&json!({"dados": {"id": 7}}))).await;
        let value: Value = quick_client(&base).get("deputados/7").await.unwrap();
        assert_eq!(value, json!({"id": 7}));
    }

    #[tokio::test]
    async fn retry_after_header_sets_the_wait_after_429() {
        let (base, calls) = serve(|n, _| {
            if n == 1 {
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    [("retry-after", "1")],
                    "slow",
                )
                    .into_response()
            } else {
                json_response(&json!({"dados": 1}))
            }
        })
        .await;
        let started = Instant::now();
        let value: i32 = quick_client(&base).get("x").await.unwrap();
        assert_eq!(value, 1);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(
            started.elapsed() >= Duration::from_secs(1),
            "the client must obey Retry-After"
        );
    }

    #[tokio::test]
    async fn server_error_without_retry_after_uses_the_backoff() {
        let (base, calls) = serve(|n, _| {
            if n < 3 {
                StatusCode::BAD_GATEWAY.into_response()
            } else {
                json_response(&json!({"dados": "ok"}))
            }
        })
        .await;
        let value: String = quick_client(&base).get("x").await.unwrap();
        assert_eq!(value, "ok");
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn retries_stop_after_the_maximum_attempts() {
        let (base, calls) = serve(|_, _| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                [("retry-after", "0")],
                "down",
            )
                .into_response()
        })
        .await;
        let err = quick_client(&base).get::<Value>("x").await.unwrap_err();
        assert!(
            matches!(err, CamaraError::Status { status: 503, ref url } if url.ends_with("/api/v2/x")),
            "{err}"
        );
        assert_eq!(calls.load(Ordering::SeqCst), MAX_ATTEMPTS as usize);
    }

    #[tokio::test]
    async fn client_error_is_not_retried() {
        let (base, calls) = serve(|_, _| StatusCode::NOT_FOUND.into_response()).await;
        let err = quick_client(&base).get::<Value>("x").await.unwrap_err();
        assert!(
            matches!(err, CamaraError::Status { status: 404, .. }),
            "{err}"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn body_that_is_not_json_is_an_http_error() {
        let (base, _) = serve(|_, _| "not json".into_response()).await;
        let err = quick_client(&base).get::<Value>("x").await.unwrap_err();
        assert!(matches!(err, CamaraError::Http(_)), "{err}");
    }

    #[tokio::test]
    async fn connection_failure_is_retried_then_returned() {
        // Bind a port, then close it: nobody listens there.
        let listener = tokio::net::TcpListener::bind("[::1]:0").await.unwrap();
        let base = format!("http://{}/api/v2", listener.local_addr().unwrap());
        drop(listener);
        let err = quick_client(&base).get::<Value>("x").await.unwrap_err();
        assert!(
            matches!(&err, CamaraError::Http(e) if e.is_connect()),
            "{err}"
        );
    }

    #[tokio::test]
    async fn pace_keeps_the_minimum_interval_between_requests() {
        let (base, _) = serve(|_, _| json_response(&json!({"dados": 1}))).await;
        let mut client = quick_client(&base);
        client.min_interval = Duration::from_millis(200);
        let started = Instant::now();
        for _ in 0..3 {
            let _value: i32 = client.get("x").await.unwrap();
        }
        assert!(started.elapsed() >= Duration::from_millis(400));
    }

    #[tokio::test]
    async fn list_follows_next_links_and_rebases_absolute_ones() {
        let (base, calls) = serve(|n, target| match n {
            1 => {
                assert!(target.ends_with("/deputados?uf=SP&itens=100"), "{target}");
                // The real API sends absolute links on the public host.
                let next = format!("{BASE_URL}/deputados?uf=SP&itens=100&pagina=2");
                json_response(&json!({"dados": [1, 2], "links": [
                    {"rel": "self", "href": "ignored"}, {"rel": "next", "href": next}]}))
            }
            _ => {
                assert!(target.ends_with("pagina=2"), "{target}");
                json_response(&json!({"dados": [3]}))
            }
        })
        .await;
        let items: Vec<i32> = quick_client(&base).list("deputados?uf=SP").await.unwrap();
        assert_eq!(items, vec![1, 2, 3]);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn list_without_a_query_adds_the_page_size() {
        let (base, _) = serve(|_, target| {
            assert!(target.ends_with("/partidos?itens=100"), "{target}");
            json_response(&json!({"dados": []}))
        })
        .await;
        let items: Vec<i32> = quick_client(&base).list("partidos").await.unwrap();
        assert!(items.is_empty());
    }

    #[tokio::test]
    async fn list_stops_at_the_page_limit() {
        // Every page links to a next page: a loop that the client must cut.
        let (base, calls) = serve(|_, _| {
            let next = format!("{BASE_URL}/loop");
            json_response(&json!({"dados": [1], "links": [{"rel": "next", "href": next}]}))
        })
        .await;
        let client = quick_client(&base);
        let items: Vec<i32> = client.list("x").await.unwrap();
        assert_eq!(items.len(), MAX_PAGES);
        assert_eq!(calls.load(Ordering::SeqCst), MAX_PAGES);
    }

    #[test]
    fn rebase_leaves_other_hosts_alone() {
        let client = CamaraClient::with_base("http://[::1]:9/api/v2").unwrap();
        assert_eq!(
            client.rebase("http://other.example/x"),
            "http://other.example/x"
        );
        assert_eq!(
            client.rebase(&format!("{BASE_URL}/y?z=1")),
            "http://[::1]:9/api/v2/y?z=1"
        );
    }
}
