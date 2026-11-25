//! Campaign finance of the deputies in office, from the TSE (ADR-0009).
//!
//! The import is a sync job (`donations`) that only an operator starts. It does not run on
//! a schedule: the 2022 accounts do not change. The job:
//! 1. gets the CPF of each deputy in office from the Câmara API (in memory only);
//! 2. downloads the TSE archive to a temporary file (about 500 MB) and streams the CSV;
//! 3. replaces all rows of the election year in one transaction.
//!
//! DIOGENES shows declared facts with their source. It does not compute a judgement
//! ("influenced", "lobbyist") about a person.

pub mod api;
pub mod parse;

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use sha2::{Digest, Sha256};
use sqlx::PgPool;

use crate::camara::dto::DeputyDetail;
use crate::camara::CamaraClient;
use crate::sync::{Check, JobError, JobReport};
use crate::tse::TseError;
use parse::{OriginalDonation, Receipt, Rejected, Source};

/// The election of the deputies in office (57th legislature).
pub const ELECTION_YEAR: i16 = 2022;
/// The TSE archive with the accounts of all 2022 candidates.
pub const FINANCE_2022_URL: &str = "https://cdn.tse.jus.br/estatistica/sead/odsele/prestacao_contas/prestacao_de_contas_eleitorais_candidatos_2022.zip";
/// The public page of the data set. The interface links to it.
pub const SOURCE_PAGE: &str =
    "https://dadosabertos.tse.jus.br/dataset/prestacao-de-contas-eleitorais-2022";
/// The largest archive that the import downloads. The 2022 archive is about 500 MB.
pub const MAX_DOWNLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(1800);
/// The length of a donor key (hex characters).
const KEY_LEN: usize = 24;

fn io(err: &std::io::Error) -> TseError {
    TseError::Archive(err.to_string())
}

/// Download `url` to `path`. Refuse an archive larger than `max_bytes`; never truncate.
///
/// # Errors
/// Returns an error when the download fails, the file cannot be written, or the archive is
/// too large.
pub async fn download(url: &str, path: &Path, max_bytes: u64) -> Result<u64, TseError> {
    let http = reqwest::Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .build()?;
    let mut response = http.get(url).send().await?.error_for_status()?;
    let too_large = || TseError::Archive(format!("archive larger than {max_bytes} bytes"));
    if response.content_length().is_some_and(|n| n > max_bytes) {
        return Err(too_large());
    }
    let mut file = std::fs::File::create(path).map_err(|e| io(&e))?;
    let mut total: u64 = 0;
    while let Some(chunk) = response.chunk().await? {
        total += chunk.len() as u64;
        if total > max_bytes {
            return Err(too_large());
        }
        file.write_all(&chunk).map_err(|e| io(&e))?;
    }
    file.flush().map_err(|e| io(&e))?;
    Ok(total)
}

/// The rows of the archive that belong to the deputies in office.
#[derive(Debug, Default)]
pub struct Parsed {
    /// The receipts.
    pub receipts: Vec<Receipt>,
    /// The original donors of party money.
    pub originals: Vec<OriginalDonation>,
    /// The rows that could not be read.
    pub rejected: Rejected,
}

/// Read the two national files of the archive at `path`.
///
/// # Errors
/// Returns an error when the archive or a file in it is not valid.
pub fn read_archive(path: &Path, cpfs: &HashMap<String, i32>) -> Result<Parsed, TseError> {
    let file = std::fs::File::open(path).map_err(|e| io(&e))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| TseError::Archive(e.to_string()))?;
    let mut rejected = Rejected::default();
    let receipts = {
        let entry = archive
            .by_name(parse::RECEIPTS_FILE)
            .map_err(|e| TseError::Archive(e.to_string()))?;
        parse::receipts(entry, cpfs, &mut rejected)?
    };
    let party: HashMap<String, i32> = receipts
        .iter()
        .filter(|r| r.source == Source::Party)
        .map(|r| (r.seq.clone(), r.camara_id))
        .collect();
    let originals = {
        let entry = archive
            .by_name(parse::ORIGINAL_DONORS_FILE)
            .map_err(|e| TseError::Archive(e.to_string()))?;
        parse::original_donors(entry, &party, &mut rejected)?
    };
    Ok(Parsed {
        receipts,
        originals,
        rejected,
    })
}

/// A donor key: a hash of the document with a salt that exists only during one import.
fn donor_key(salt: &[u8], doc: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(salt);
    hasher.update(doc.as_bytes());
    let digest = hasher.finalize();
    digest
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
        .chars()
        .take(KEY_LEN)
        .collect()
}

/// One donor row to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DonorRow {
    /// `person` or `candidate`.
    pub kind: &'static str,
    /// The name.
    pub name: String,
    /// The economic activity.
    pub activity: Option<String>,
    /// The state.
    pub uf: Option<String>,
    /// The six middle digits of the CPF of a person (the part that the Receita publishes).
    pub cpf_part: Option<String>,
}

/// The rows to write, grouped.
#[derive(Debug, Default)]
pub struct Rows {
    /// (Câmara id, source) → (cents, receipts).
    pub totals: HashMap<(i32, Source), (i64, i32)>,
    /// key → donor.
    pub donors: HashMap<String, DonorRow>,
    /// (Câmara id, key, via party) → (cents, receipts).
    pub donations: HashMap<(i32, String, bool), (i64, i32)>,
}

/// Group the parsed rows. `salt` makes the donor keys.
#[must_use]
pub fn group(parsed: &Parsed, salt: &[u8]) -> Rows {
    let mut rows = Rows::default();
    let mut donor = |d: &parse::Donor| {
        let key = donor_key(salt, &d.doc);
        rows.donors.entry(key.clone()).or_insert_with(|| DonorRow {
            kind: d.kind.key(),
            name: d.name.clone(),
            activity: d.activity.clone(),
            uf: d.uf.clone(),
            cpf_part: (d.kind == parse::DonorKind::Person)
                .then(|| parse::cpf_part(&d.doc))
                .flatten(),
        });
        key
    };
    let mut donations: HashMap<(i32, String, bool), (i64, i32)> = HashMap::new();
    for r in &parsed.receipts {
        let t = rows.totals.entry((r.camara_id, r.source)).or_default();
        t.0 += r.cents;
        t.1 += 1;
        if let Some(d) = &r.donor {
            let e = donations.entry((r.camara_id, donor(d), false)).or_default();
            e.0 += r.cents;
            e.1 += 1;
        }
    }
    for o in &parsed.originals {
        let e = donations
            .entry((o.camara_id, donor(&o.donor), true))
            .or_default();
        e.0 += o.cents;
        e.1 += 1;
    }
    rows.donations = donations;
    rows
}

/// Replace all rows of `year` in one transaction.
///
/// # Errors
/// Returns a database error.
pub async fn write(db: &PgPool, year: i16, rows: &Rows) -> Result<(), sqlx::Error> {
    let mut tx = db.begin().await?;
    for table in [
        "diogenes_donation",
        "diogenes_donor",
        "diogenes_campaign_total",
    ] {
        sqlx::query(&format!("DELETE FROM {table} WHERE election_year = $1"))
            .bind(year)
            .execute(&mut *tx)
            .await?;
    }
    let (ids, sources, cents, counts): (Vec<i32>, Vec<&str>, Vec<i64>, Vec<i32>) = rows
        .totals
        .iter()
        .map(|((id, s), (c, n))| (*id, s.key(), *c, *n))
        .fold(Default::default(), |mut acc, (a, b, c, d)| {
            acc.0.push(a);
            acc.1.push(b);
            acc.2.push(c);
            acc.3.push(d);
            acc
        });
    sqlx::query(
        "INSERT INTO diogenes_campaign_total (election_year, camara_id, source, cents, receipts)
         SELECT $1, * FROM UNNEST($2::int[], $3::text[], $4::bigint[], $5::int[])",
    )
    .bind(year)
    .bind(&ids)
    .bind(&sources)
    .bind(&cents)
    .bind(&counts)
    .execute(&mut *tx)
    .await?;
    let mut keys = Vec::new();
    let mut kinds = Vec::new();
    let mut names = Vec::new();
    let mut activities = Vec::new();
    let mut ufs = Vec::new();
    let mut parts = Vec::new();
    for (key, d) in &rows.donors {
        keys.push(key.as_str());
        kinds.push(d.kind);
        names.push(d.name.as_str());
        activities.push(d.activity.as_deref());
        ufs.push(d.uf.as_deref());
        parts.push(d.cpf_part.as_deref());
    }
    sqlx::query(
        "INSERT INTO diogenes_donor (election_year, key, kind, name, activity, uf, cpf_part)
         SELECT $1, * FROM UNNEST($2::text[], $3::text[], $4::text[], $5::text[], $6::text[],
                                  $7::text[])",
    )
    .bind(year)
    .bind(&keys)
    .bind(&kinds)
    .bind(&names)
    .bind(&activities)
    .bind(&ufs)
    .bind(&parts)
    .execute(&mut *tx)
    .await?;
    let mut ids = Vec::new();
    let mut keys = Vec::new();
    let mut via = Vec::new();
    let mut cents = Vec::new();
    let mut counts = Vec::new();
    for ((id, key, v), (c, n)) in &rows.donations {
        ids.push(*id);
        keys.push(key.as_str());
        via.push(*v);
        cents.push(*c);
        counts.push(*n);
    }
    sqlx::query(
        "INSERT INTO diogenes_donation (election_year, camara_id, donor_key, via_party, cents, receipts)
         SELECT $1, * FROM UNNEST($2::int[], $3::text[], $4::bool[], $5::bigint[], $6::int[])",
    )
    .bind(year)
    .bind(&ids)
    .bind(&keys)
    .bind(&via)
    .bind(&cents)
    .bind(&counts)
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

/// A deputy in office with the CPF and the civil name from the Câmara API. The CPF stays in
/// memory only.
#[derive(Debug, Clone)]
pub struct DeputyPerson {
    /// The Câmara id.
    pub camara_id: i32,
    /// The CPF, 11 digits. Empty when the API gives no valid CPF.
    pub cpf: String,
    /// The civil name.
    pub civil_name: String,
}

/// Read the CPF and the civil name of each deputy in office from the Câmara API.
///
/// # Errors
/// Returns the first database or API error.
pub async fn deputy_people(
    db: &PgPool,
    client: &CamaraClient,
) -> Result<Vec<DeputyPerson>, JobError> {
    let in_office: Vec<i32> =
        sqlx::query_scalar("SELECT camara_id FROM diogenes_deputy WHERE in_office ORDER BY 1")
            .fetch_all(db)
            .await?;
    let mut out = Vec::with_capacity(in_office.len());
    for id in in_office {
        let detail: DeputyDetail = client.get(&format!("deputados/{id}")).await?;
        let cpf: String = detail
            .cpf
            .as_deref()
            .unwrap_or_default()
            .chars()
            .filter(char::is_ascii_digit)
            .collect();
        out.push(DeputyPerson {
            camara_id: id,
            cpf: if cpf.len() == 11 { cpf } else { String::new() },
            civil_name: detail.nome_civil.unwrap_or_default(),
        });
    }
    Ok(out)
}

/// The `donations` job: import the accounts of `url` for the deputies in office.
///
/// # Errors
/// Returns the first API, archive or database error. A failed run changes nothing.
pub async fn run(db: &PgPool, client: &CamaraClient, url: &str) -> Result<JobReport, JobError> {
    let people = deputy_people(db, client).await?;
    let in_office = people.len();
    let cpfs: HashMap<String, i32> = people
        .into_iter()
        .filter(|p| !p.cpf.is_empty())
        .map(|p| (p.cpf, p.camara_id))
        .collect();
    let path = std::env::temp_dir().join(format!("diogenes-tse-{}.zip", uuid::Uuid::now_v7()));
    let parsed = async {
        download(url, &path, MAX_DOWNLOAD_BYTES).await?;
        let p = path.clone();
        tokio::task::spawn_blocking(move || read_archive(&p, &cpfs))
            .await
            .map_err(|e| TseError::Archive(e.to_string()))?
    }
    .await;
    // The archive is large: remove it in all cases.
    let _ = std::fs::remove_file(&path);
    let parsed = parsed?;
    let salt = [
        uuid::Uuid::new_v4().into_bytes(),
        uuid::Uuid::new_v4().into_bytes(),
    ]
    .concat();
    let rows = group(&parsed, &salt);
    write(db, ELECTION_YEAR, &rows).await?;
    let matched = rows
        .totals
        .keys()
        .map(|(id, _)| *id)
        .collect::<std::collections::HashSet<_>>()
        .len();
    Ok(JobReport {
        items: parsed.receipts.len() as i64 + parsed.originals.len() as i64,
        checks: vec![
            Check {
                subject: "deputies_with_accounts".into(),
                expected: in_office as i64,
                actual: matched as i64,
            },
            Check {
                subject: "rejected_rows".into(),
                expected: 0,
                actual: parsed.rejected.bad_row + parsed.rejected.bad_value,
            },
        ],
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use parse::{Donor, DonorKind};

    fn receipt(id: i32, source: Source, doc: Option<&str>, cents: i64) -> Receipt {
        Receipt {
            camara_id: id,
            seq: "1".into(),
            source,
            donor: doc.map(|d| Donor {
                doc: d.into(),
                kind: DonorKind::Person,
                name: format!("N{d}"),
                activity: None,
                uf: Some("SP".into()),
            }),
            cents,
        }
    }

    #[test]
    fn keys_are_stable_in_one_import_and_change_with_the_salt() {
        let a = donor_key(b"salt-1", "22222222222");
        assert_eq!(a, donor_key(b"salt-1", "22222222222"));
        assert_ne!(a, donor_key(b"salt-2", "22222222222"));
        assert_ne!(a, donor_key(b"salt-1", "22222222223"));
        assert_eq!(a.len(), KEY_LEN);
        assert!(!a.contains("22222222222"));
    }

    #[test]
    fn group_sums_totals_and_donations() {
        let parsed = Parsed {
            receipts: vec![
                receipt(1, Source::Individuals, Some("2"), 100),
                receipt(1, Source::Individuals, Some("2"), 50),
                receipt(2, Source::Individuals, Some("2"), 10),
                receipt(1, Source::Party, None, 1000),
            ],
            originals: vec![OriginalDonation {
                camara_id: 1,
                donor: Donor {
                    doc: "2".into(),
                    kind: DonorKind::Person,
                    name: "N2".into(),
                    activity: None,
                    uf: None,
                },
                cents: 400,
            }],
            rejected: Rejected::default(),
        };
        let rows = group(&parsed, b"s");
        assert_eq!(rows.totals[&(1, Source::Individuals)], (150, 2));
        assert_eq!(rows.totals[&(1, Source::Party)], (1000, 1));
        assert_eq!(rows.donors.len(), 1, "one person, one key");
        let key = rows.donors.keys().next().unwrap().clone();
        assert_eq!(rows.donations[&(1, key.clone(), false)], (150, 2));
        assert_eq!(rows.donations[&(2, key.clone(), false)], (10, 1));
        assert_eq!(rows.donations[&(1, key, true)], (400, 1));
    }
}
