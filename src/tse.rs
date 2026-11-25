//! The TSE candidate file. Source of self-declared colour/race ("cor/raça").
//!
//! The Câmara API has no colour/race field. The TSE publishes the self-declaration of
//! each candidate, with the CPF. The sync joins the two sources by CPF. The CPF map
//! lives only in memory during the sync. DIOGENES stores the race, not the CPF.

use std::collections::HashMap;
use std::io::{Cursor, Read};

/// The candidate file of the 2022 general election.
pub const CANDIDATES_2022_URL: &str =
    "https://cdn.tse.jus.br/estatistica/sead/odsele/consulta_cand/consulta_cand_2022.zip";
const NATIONAL_FILE: &str = "consulta_cand_2022_BRASIL.csv";
const OFFICE: &str = "DEPUTADO FEDERAL";
const DOWNLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);
/// The largest archive that the import downloads (the 2022 file is approximately 5 MB).
const MAX_DOWNLOAD_BYTES: usize = 64 * 1024 * 1024;
/// The largest CSV that the import reads from the archive.
const MAX_CSV_BYTES: u64 = 256 * 1024 * 1024;

/// An error of the TSE import.
#[derive(Debug, thiserror::Error)]
pub enum TseError {
    /// The download failed.
    #[error("download: {0}")]
    Download(#[from] reqwest::Error),
    /// The archive is not valid, or it has no national file.
    #[error("archive: {0}")]
    Archive(String),
    /// The CSV has no header or no expected column.
    #[error("csv: {0}")]
    Csv(String),
}

/// Download the 2022 file and return the CPF → race map of federal deputy candidates.
///
/// # Errors
/// Returns an error when the download, the archive or the CSV fails.
pub async fn fetch_races_2022() -> Result<HashMap<String, String>, TseError> {
    fetch_races(CANDIDATES_2022_URL, MAX_DOWNLOAD_BYTES).await
}

/// Download an archive from `url` and return the CPF → race map. Tests use a local server.
async fn fetch_races(url: &str, max_bytes: usize) -> Result<HashMap<String, String>, TseError> {
    let http = reqwest::Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .build()?;
    let bytes = http
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    if bytes.len() > max_bytes {
        return Err(TseError::Archive(format!(
            "archive larger than {max_bytes} bytes"
        )));
    }
    let csv = tokio::task::spawn_blocking(move || extract_national(&bytes))
        .await
        .map_err(|e| TseError::Archive(e.to_string()))??;
    parse_races(&csv)
}

fn extract_national(zip_bytes: &[u8]) -> Result<Vec<u8>, TseError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(zip_bytes))
        .map_err(|e| TseError::Archive(e.to_string()))?;
    let file = archive
        .by_name(NATIONAL_FILE)
        .map_err(|e| TseError::Archive(e.to_string()))?;
    let mut out = Vec::new();
    file.take(MAX_CSV_BYTES)
        .read_to_end(&mut out)
        .map_err(|e| TseError::Archive(e.to_string()))?;
    Ok(out)
}

/// Decode ISO-8859-1 bytes. Each byte is the Unicode code point with the same value.
fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|b| char::from(*b)).collect()
}

/// Parse the CSV (latin-1, `;` separator) into a CPF → race map.
///
/// # Errors
/// Returns an error when a required column is missing.
pub fn parse_races(csv_bytes: &[u8]) -> Result<HashMap<String, String>, TseError> {
    let text = latin1(csv_bytes);
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(b';')
        .flexible(true)
        .from_reader(text.as_bytes());
    let headers = reader
        .headers()
        .map_err(|e| TseError::Csv(e.to_string()))?
        .clone();
    let col = |name: &str| {
        headers
            .iter()
            .position(|h| h == name)
            .ok_or_else(|| TseError::Csv(format!("missing column {name}")))
    };
    let (office, cpf, race) = (
        col("DS_CARGO")?,
        col("NR_CPF_CANDIDATO")?,
        col("DS_COR_RACA")?,
    );
    let mut map = HashMap::new();
    for record in reader.records().filter_map(Result::ok) {
        if !record
            .get(office)
            .is_some_and(|o| o.eq_ignore_ascii_case(OFFICE))
        {
            continue;
        }
        let (Some(c), Some(r)) = (record.get(cpf), record.get(race)) else {
            continue;
        };
        let c: String = c.chars().filter(char::is_ascii_digit).collect();
        if c.len() == 11 && !r.trim().is_empty() {
            map.insert(c, r.trim().to_uppercase());
        }
    }
    Ok(map)
}

/// True when a race value counts as Black ("negra") in the IBGE sense: PRETA or PARDA.
#[must_use]
pub fn is_black_or_brown(race: &str) -> bool {
    matches!(race, "PRETA" | "PARDA")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::io::Write;

    const CSV: &[u8] = b"\"DS_CARGO\";\"NR_CPF_CANDIDATO\";\"DS_COR_RACA\"\n\
        \"DEPUTADO FEDERAL\";\"123.456.789-01\";\" preta \"\n";

    /// A zip archive in memory, with one stored file.
    fn zip_with(name: &str, content: &[u8]) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(&mut out);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        writer.start_file(name, options).unwrap();
        writer.write_all(content).unwrap();
        writer.finish().unwrap();
        out.into_inner()
    }

    /// Serve `body` with `status` on `[::1]` for any path. Return the URL.
    async fn serve(status: u16, body: Vec<u8>) -> String {
        let app = axum::Router::new().fallback(move || {
            let body = body.clone();
            async move { (axum::http::StatusCode::from_u16(status).unwrap(), body) }
        });
        let listener = tokio::net::TcpListener::bind("[::1]:0").await.unwrap();
        let url = format!("http://{}/file.zip", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        url
    }

    #[tokio::test]
    async fn downloads_the_archive_and_maps_cpf_to_race() {
        let url = serve(200, zip_with(NATIONAL_FILE, CSV)).await;
        let map = fetch_races(&url, 1024 * 1024).await.unwrap();
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("12345678901").map(String::as_str), Some("PRETA"));
    }

    #[tokio::test]
    async fn http_error_status_is_a_download_error() {
        let url = serve(404, b"gone".to_vec()).await;
        let err = fetch_races(&url, 1024).await.unwrap_err();
        assert!(matches!(err, TseError::Download(_)), "{err}");
    }

    #[tokio::test]
    async fn archive_over_the_size_limit_is_refused() {
        let archive = zip_with(NATIONAL_FILE, CSV);
        let limit = archive.len() - 1;
        let url = serve(200, archive).await;
        let err = fetch_races(&url, limit).await.unwrap_err();
        assert!(
            matches!(&err, TseError::Archive(m) if m.contains("larger than")),
            "{err}"
        );
    }

    #[tokio::test]
    async fn bytes_that_are_not_a_zip_are_an_archive_error() {
        let url = serve(200, b"this is not a zip".to_vec()).await;
        let err = fetch_races(&url, 1024).await.unwrap_err();
        assert!(matches!(err, TseError::Archive(_)), "{err}");
    }

    #[tokio::test]
    async fn archive_without_the_national_file_is_an_archive_error() {
        let url = serve(200, zip_with("consulta_cand_2022_SP.csv", CSV)).await;
        let err = fetch_races(&url, 1024 * 1024).await.unwrap_err();
        assert!(matches!(err, TseError::Archive(_)), "{err}");
    }

    #[tokio::test]
    async fn national_file_without_the_race_column_is_a_csv_error() {
        let url = serve(200, zip_with(NATIONAL_FILE, b"DS_CARGO;NR_CPF_CANDIDATO\n")).await;
        let err = fetch_races(&url, 1024 * 1024).await.unwrap_err();
        assert!(
            matches!(&err, TseError::Csv(m) if m.contains("DS_COR_RACA")),
            "{err}"
        );
    }

    #[test]
    fn rows_without_a_race_or_with_a_short_row_are_skipped() {
        let csv = b"DS_CARGO;NR_CPF_CANDIDATO;DS_COR_RACA\n\
            DEPUTADO FEDERAL;12345678901;  \n\
            DEPUTADO FEDERAL;12345678902\n\
            deputado federal;12345678903;branca\n";
        let map = parse_races(csv).unwrap();
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("12345678903").map(String::as_str), Some("BRANCA"));
    }

    #[test]
    fn parses_only_federal_deputies_and_decodes_latin1() {
        let mut csv = b"\"DS_CARGO\";\"NR_CPF_CANDIDATO\";\"DS_COR_RACA\"\n".to_vec();
        csv.extend_from_slice(b"\"DEPUTADO FEDERAL\";\"12345678901\";\"PARDA\"\n");
        csv.extend_from_slice(b"\"SENADOR\";\"99999999999\";\"BRANCA\"\n");
        csv.extend_from_slice(b"\"DEPUTADO FEDERAL\";\"1\";\"PRETA\"\n");
        // "IND\xcdGENA" is latin-1 for "INDÍGENA".
        csv.extend_from_slice(b"\"DEPUTADO FEDERAL\";\"10987654321\";\"IND\xcdGENA\"\n");
        let map = parse_races(&csv).unwrap_or_default();
        assert_eq!(map.len(), 2);
        assert_eq!(map.get("12345678901").map(String::as_str), Some("PARDA"));
        assert_eq!(map.get("10987654321").map(String::as_str), Some("INDÍGENA"));
    }

    #[test]
    fn missing_column_is_an_error() {
        assert!(parse_races(b"A;B\n1;2\n").is_err());
    }

    #[test]
    fn black_or_brown_follows_ibge() {
        assert!(is_black_or_brown("PRETA"));
        assert!(is_black_or_brown("PARDA"));
        assert!(!is_black_or_brown("BRANCA"));
    }
}
