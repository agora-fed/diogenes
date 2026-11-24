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
        })
    }

    async fn pace(&self) {
        let mut last = self.last.lock().await;
        if let Some(at) = *last {
            let elapsed = at.elapsed();
            if elapsed < MIN_INTERVAL {
                tokio::time::sleep(MIN_INTERVAL - elapsed).await;
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
                    tokio::time::sleep(BASE_BACKOFF * attempt).await;
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
                .map_or(BASE_BACKOFF * attempt, Duration::from_secs);
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
