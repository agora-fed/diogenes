//! Companies of the donors and of the deputies (Receita Federal CNPJ open data, ADR-0010).
//!
//! The `companies` job:
//! 1. finds the month to read (the configured month, or the last month of the share);
//! 2. reads the 10 files of partners and keeps the people whose normalized name and six
//!    middle CPF digits are equal to a person donor or to a deputy in office;
//! 3. reads the 10 files of companies and the 10 files of establishments, and keeps the
//!    rows of the companies from step 2 (head office only);
//! 4. reads the code tables, and replaces all rows in one transaction.
//!
//! The parts stream from the HTTP body to the reader. No part goes to the disk.

pub mod api;
pub mod parse;

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::time::Duration;

use bytes::Bytes;
use chrono::NaiveDate;
use sqlx::PgPool;
use tokio::sync::mpsc;

use crate::camara::CamaraClient;
use crate::finance::{deputy_people, ELECTION_YEAR};
use crate::sync::{Check, JobError, JobReport};
use crate::text::normalize;
use parse::{CnpjError, Company, Counts, HeadOffice, Partner};

/// The WebDAV root of the public share of the Receita.
pub const DEFAULT_BASE: &str = "https://arquivos.receitafederal.gov.br/public.php/webdav";
/// The token of the public share on the Receita server, where the CNPJ files are
/// (checked on 2026-11-25). If the Receita moves the files, set `DIOGENES_RFB_CNPJ_SHARE`.
pub const DEFAULT_SHARE: &str = "YggdBLfdninEJX9";
/// The public page of the data set.
pub const SOURCE_PAGE: &str =
    "https://dados.gov.br/dados/conjuntos-dados/cadastro-nacional-da-pessoa-juridica---cnpj";
/// The name of the data set in `diogenes_dataset`.
pub const DATASET: &str = "rfb_cnpj";
/// The number of parts of each large file.
pub const PARTS: usize = 10;
const READ_TIMEOUT: Duration = Duration::from_secs(120);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// Chunks in flight between the HTTP body and the reader.
const CHANNEL_CHUNKS: usize = 64;

/// Where the job reads the CNPJ files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CnpjSource {
    /// The WebDAV root (`DIOGENES_RFB_CNPJ_URL`).
    pub base: String,
    /// The share token, sent as the user name (`DIOGENES_RFB_CNPJ_SHARE`).
    pub share: String,
    /// The month (`YYYY-MM`, `DIOGENES_RFB_CNPJ_MONTH`). `None`: the last month.
    pub month: Option<String>,
}

impl Default for CnpjSource {
    fn default() -> Self {
        Self {
            base: DEFAULT_BASE.to_owned(),
            share: DEFAULT_SHARE.to_owned(),
            month: None,
        }
    }
}

impl CnpjSource {
    fn url(&self, path: &str) -> String {
        format!("{}/{path}", self.base.trim_end_matches('/'))
    }
}

impl From<CnpjError> for JobError {
    fn from(err: CnpjError) -> Self {
        Self::Cnpj(err.to_string())
    }
}

impl From<reqwest::Error> for JobError {
    fn from(err: reqwest::Error) -> Self {
        Self::Cnpj(err.without_url().to_string())
    }
}

/// Find the months (`YYYY-MM`) in a WebDAV listing, last month first.
#[must_use]
pub fn months(listing: &str) -> Vec<String> {
    let mut out: Vec<String> = listing
        .split("<d:href>")
        .skip(1)
        .filter_map(|h| h.split('<').next())
        .filter_map(|h| h.trim_end_matches('/').rsplit('/').next())
        .filter(|m| {
            let b = m.as_bytes();
            b.len() == 7 && b[4] == b'-' && m.chars().filter(char::is_ascii_digit).count() == 6
        })
        .map(str::to_owned)
        .collect();
    out.sort_unstable_by(|a, b| b.cmp(a));
    out.dedup();
    out
}

async fn last_month(http: &reqwest::Client, source: &CnpjSource) -> Result<String, JobError> {
    let method =
        reqwest::Method::from_bytes(b"PROPFIND").map_err(|e| JobError::Cnpj(e.to_string()))?;
    let listing = http
        .request(method, source.url(""))
        .basic_auth(&source.share, Some(""))
        .header("Depth", "1")
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    months(&listing)
        .into_iter()
        .next()
        .ok_or_else(|| JobError::Cnpj("the share has no month".into()))
}

/// A `Read` over the chunks of an HTTP body. It runs on a blocking thread.
struct BodyReader {
    rx: mpsc::Receiver<Result<Bytes, String>>,
    chunk: Bytes,
}

impl Read for BodyReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        while self.chunk.is_empty() {
            match self.rx.blocking_recv() {
                Some(Ok(b)) => self.chunk = b,
                Some(Err(e)) => return Err(std::io::Error::other(e)),
                None => return Ok(0),
            }
        }
        let n = buf.len().min(self.chunk.len());
        buf[..n].copy_from_slice(&self.chunk.split_to(n));
        Ok(n)
    }
}

/// Stream one ZIP part from `url` and call `f` for each CSV row. `state` goes to the
/// blocking reader and comes back with the counts.
async fn read_part<S, F>(
    http: &reqwest::Client,
    source: &CnpjSource,
    url: String,
    state: S,
    f: F,
) -> Result<(S, Counts), JobError>
where
    S: Send + 'static,
    F: Fn(&mut S, &[String]) + Send + 'static,
{
    let mut response = http
        .get(&url)
        .basic_auth(&source.share, Some(""))
        .send()
        .await?
        .error_for_status()?;
    let (tx, rx) = mpsc::channel(CHANNEL_CHUNKS);
    let reader = tokio::task::spawn_blocking(move || {
        let mut state = state;
        let body = BodyReader {
            rx,
            chunk: Bytes::new(),
        };
        let counts = parse::each_row(body, |row| f(&mut state, row))?;
        Ok::<_, CnpjError>((state, counts))
    });
    loop {
        match response.chunk().await {
            Ok(Some(b)) => {
                // The reader stops early only on an error, which it returns below.
                if tx.send(Ok(b)).await.is_err() {
                    break;
                }
            }
            Ok(None) => break,
            Err(e) => {
                let _ = tx.send(Err(e.without_url().to_string())).await;
                break;
            }
        }
    }
    drop(tx);
    let result = reader.await.map_err(|e| JobError::Cnpj(e.to_string()))??;
    Ok(result)
}

/// A person to find in the file of partners.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Target {
    /// A donor (key of the import).
    Donor(String),
    /// A deputy in office.
    Deputy(i32),
}

/// The people to find, by (normalized name, six CPF digits). A key with two or more
/// different donors is ambiguous: the job does not use it.
#[derive(Debug, Default)]
pub struct Targets {
    by_key: HashMap<(String, String), Vec<Target>>,
    names: HashSet<String>,
    /// The keys that have two or more donors.
    pub ambiguous: usize,
}

impl Targets {
    /// Make the targets. `donors`: (key, name, six digits). `deputies`: (id, civil name, CPF).
    #[must_use]
    pub fn new(donors: &[(String, String, String)], deputies: &[(i32, String, String)]) -> Self {
        let mut by_key: HashMap<(String, String), Vec<Target>> = HashMap::new();
        for (key, name, part) in donors {
            by_key
                .entry((normalize(name), part.clone()))
                .or_default()
                .push(Target::Donor(key.clone()));
        }
        let ambiguous_keys: Vec<_> = by_key
            .iter()
            .filter(|(_, v)| v.len() > 1)
            .map(|(k, _)| k.clone())
            .collect();
        for k in &ambiguous_keys {
            by_key.remove(k);
        }
        for (id, name, cpf) in deputies {
            if let Some(part) = crate::finance::parse::cpf_part(cpf) {
                if !name.trim().is_empty() {
                    by_key
                        .entry((normalize(name), part))
                        .or_default()
                        .push(Target::Deputy(*id));
                }
            }
        }
        let names = by_key.keys().map(|(n, _)| n.clone()).collect();
        Self {
            by_key,
            names,
            ambiguous: ambiguous_keys.len(),
        }
    }

    /// The normalized name of a partner, when a target has this name.
    #[must_use]
    pub fn known_name(&self, partner: &Partner) -> Option<String> {
        let name = normalize(&partner.name);
        self.names.contains(&name).then_some(name)
    }

    /// The targets with this normalized name and these six digits.
    #[must_use]
    pub fn find(&self, name: &str, cpf_part: &str) -> &[Target] {
        self.by_key
            .get(&(name.to_owned(), cpf_part.to_owned()))
            .map_or(&[], Vec::as_slice)
    }
}

/// One link between a person and a company.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// The person.
    pub target: Target,
    /// The company.
    pub cnpj_basico: String,
    /// The code of the qualification.
    pub role: String,
    /// The entry date.
    pub since: Option<NaiveDate>,
    /// The other people with the same name in the partner file.
    pub namesakes: i32,
}

/// The state of the pass over the partner files: the targets, the links found (with the
/// normalized name), and the six digits of all people with a target name.
type PartnerPass = (
    Targets,
    Vec<(String, Link)>,
    HashMap<String, HashSet<String>>,
);

/// All rows that the job writes.
#[derive(Debug, Default)]
pub struct Found {
    /// The links.
    pub links: Vec<Link>,
    /// The companies, by CNPJ base.
    pub companies: HashMap<String, Company>,
    /// The head offices, by CNPJ base.
    pub offices: HashMap<String, HeadOffice>,
    /// Code tables: qualification, legal nature, CNAE, city.
    pub roles: HashMap<String, String>,
    /// Legal natures.
    pub natures: HashMap<String, String>,
    /// CNAE texts.
    pub cnaes: HashMap<String, String>,
    /// City names.
    pub cities: HashMap<String, String>,
    /// The parts that the job read.
    pub files: i64,
    /// The rows that the CSV reader refused.
    pub rejected: u64,
}

impl Found {
    /// The CNPJ bases of the links.
    #[must_use]
    pub fn wanted(&self) -> HashSet<String> {
        self.links.iter().map(|l| l.cnpj_basico.clone()).collect()
    }
}

/// Read all files of `month`.
///
/// # Errors
/// Returns the first HTTP or read error. Then the job writes nothing.
pub async fn read_month(
    http: &reqwest::Client,
    source: &CnpjSource,
    month: &str,
    targets: Targets,
) -> Result<Found, JobError> {
    let mut found = Found::default();
    let part = |name: &str| source.url(&format!("{month}/{name}.zip"));
    // (name → the six digits of all people with this name), to count the namesakes.
    let mut state: PartnerPass = (targets, Vec::new(), HashMap::new());
    for i in 0..PARTS {
        let (s, counts) = read_part(
            http,
            source,
            part(&format!("Socios{i}")),
            state,
            |(t, links, people): &mut PartnerPass, row| {
                let Some(p) = parse::partner(row) else { return };
                let Some(name) = t.known_name(&p) else { return };
                for target in t.find(&name, &p.cpf_part) {
                    links.push((
                        name.clone(),
                        Link {
                            target: target.clone(),
                            cnpj_basico: p.cnpj_basico.clone(),
                            role: p.role.clone(),
                            since: p.since,
                            namesakes: 0,
                        },
                    ));
                }
                people.entry(name).or_default().insert(p.cpf_part);
            },
        )
        .await?;
        state = s;
        found.files += 1;
        found.rejected += counts.rejected;
    }
    let (_, links, people) = state;
    found.links = links
        .into_iter()
        .map(|(name, mut link)| {
            let others = people.get(&name).map_or(1, HashSet::len).saturating_sub(1);
            link.namesakes = i32::try_from(others).unwrap_or(i32::MAX);
            link
        })
        .collect();
    let wanted = std::sync::Arc::new(found.wanted());
    for i in 0..PARTS {
        let w = wanted.clone();
        let (companies, counts) = read_part(
            http,
            source,
            part(&format!("Empresas{i}")),
            HashMap::new(),
            move |out: &mut HashMap<String, Company>, row| {
                if row.first().is_some_and(|c| w.contains(c.trim())) {
                    if let Some(c) = parse::company(row) {
                        out.insert(c.cnpj_basico.clone(), c);
                    }
                }
            },
        )
        .await?;
        found.companies.extend(companies);
        found.files += 1;
        found.rejected += counts.rejected;
    }
    for i in 0..PARTS {
        let w = wanted.clone();
        let (offices, counts) = read_part(
            http,
            source,
            part(&format!("Estabelecimentos{i}")),
            HashMap::new(),
            move |out: &mut HashMap<String, HeadOffice>, row| {
                if row.first().is_some_and(|c| w.contains(c.trim())) {
                    if let Some(h) = parse::head_office(row) {
                        out.insert(h.cnpj_basico.clone(), h);
                    }
                }
            },
        )
        .await?;
        found.offices.extend(offices);
        found.files += 1;
        found.rejected += counts.rejected;
    }
    for (name, table) in [
        ("Qualificacoes", &mut found.roles),
        ("Naturezas", &mut found.natures),
        ("Cnaes", &mut found.cnaes),
        ("Municipios", &mut found.cities),
    ] {
        let (codes, counts) = read_part(
            http,
            source,
            part(name),
            HashMap::new(),
            |out: &mut HashMap<String, String>, row| {
                if let Some((k, v)) = parse::code(row) {
                    out.insert(k, v);
                }
            },
        )
        .await?;
        *table = codes;
        found.files += 1;
        found.rejected += counts.rejected;
    }
    Ok(found)
}

fn http_client() -> Result<reqwest::Client, JobError> {
    Ok(reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .user_agent("diogenes (agora-fed)")
        .build()?)
}

/// The `companies` job.
///
/// # Errors
/// Returns the first API, HTTP, read or database error. A failed run changes nothing.
pub async fn run(
    db: &PgPool,
    client: &CamaraClient,
    source: &CnpjSource,
) -> Result<JobReport, JobError> {
    let http = http_client()?;
    let month = match &source.month {
        Some(m) => m.clone(),
        None => last_month(&http, source).await?,
    };
    let donors: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT key, name, cpf_part FROM diogenes_donor
          WHERE election_year = $1 AND kind = 'person' AND cpf_part IS NOT NULL",
    )
    .bind(ELECTION_YEAR)
    .fetch_all(db)
    .await?;
    let deputies: Vec<(i32, String, String)> = deputy_people(db, client)
        .await?
        .into_iter()
        .map(|p| (p.camara_id, p.civil_name, p.cpf))
        .collect();
    let targets = Targets::new(&donors, &deputies);
    let ambiguous = targets.ambiguous;
    tracing::info!(
        month,
        donors = donors.len(),
        "diogenes companies: reading the CNPJ files"
    );
    let found = read_month(&http, source, &month, targets).await?;
    write::write(db, &found, &month, &source.url(&month)).await?;
    let wanted = found.wanted().len() as i64;
    Ok(JobReport {
        items: found.links.len() as i64,
        checks: vec![
            Check {
                subject: "files_read".into(),
                expected: (PARTS * 3 + 4) as i64,
                actual: found.files,
            },
            Check {
                subject: "companies_with_name".into(),
                expected: wanted,
                actual: found.companies.len() as i64,
            },
            Check {
                subject: "companies_with_head_office".into(),
                expected: wanted,
                actual: found.offices.len() as i64,
            },
            Check {
                subject: "ambiguous_donors".into(),
                expected: 0,
                actual: ambiguous as i64,
            },
            Check {
                subject: "rejected_rows".into(),
                expected: 0,
                actual: found.rejected as i64,
            },
        ],
    })
}

pub mod write;
