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
    let http = reqwest::Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .build()?;
    let bytes = http
        .get(CANDIDATES_2022_URL)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    if bytes.len() > MAX_DOWNLOAD_BYTES {
        return Err(TseError::Archive(format!(
            "archive larger than {MAX_DOWNLOAD_BYTES} bytes"
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
mod tests {
    use super::*;

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
