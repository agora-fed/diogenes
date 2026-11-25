//! Read the Receita Federal CNPJ open data (ADR-0010).
//!
//! Each part is a ZIP file with one CSV entry. The CSV has no header, the separator is
//! `;`, all values have quotes, and the text is latin-1. The column positions come from
//! the Receita layout ("cnpj-metadados.pdf").
//!
//! The largest part has more than 2 GB of compressed data. The reader does not seek: it
//! reads the local header of the first entry and inflates the data that follows. Thus the
//! job can read the HTTP body directly, and no part goes to the disk.

use std::io::{BufReader, Read};

use chrono::NaiveDate;
use flate2::read::DeflateDecoder;

use crate::finance::parse::cents;

/// The signature of a ZIP local file header.
const LOCAL_HEADER: u32 = 0x0403_4b50;
/// The fixed part of a local file header, in bytes.
const LOCAL_HEADER_LEN: usize = 30;
const STORED: u16 = 0;
const DEFLATED: u16 = 8;

/// An error of the CNPJ reader.
#[derive(Debug, thiserror::Error)]
pub enum CnpjError {
    /// The data is not a ZIP file that the reader can use.
    #[error("zip: {0}")]
    Zip(String),
    /// A read failed.
    #[error("read: {0}")]
    Io(String),
}

/// The counts of one file.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    /// The rows that the reader gave to the caller.
    pub rows: u64,
    /// The rows that the CSV reader refused.
    pub rejected: u64,
}

/// Read the first entry of the ZIP data in `input` as CSV rows, and call `f` for each row.
///
/// # Errors
/// Returns an error when the data does not start with a local header, when the entry uses
/// a method other than "stored" or "deflated", or when a read fails.
pub fn each_row<R: Read>(mut input: R, mut f: impl FnMut(&[String])) -> Result<Counts, CnpjError> {
    let mut header = [0u8; LOCAL_HEADER_LEN];
    input
        .read_exact(&mut header)
        .map_err(|e| CnpjError::Zip(format!("no local header: {e}")))?;
    let le16 = |i: usize| u16::from_le_bytes([header[i], header[i + 1]]);
    let signature = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
    if signature != LOCAL_HEADER {
        return Err(CnpjError::Zip("not a zip file".into()));
    }
    let flags = le16(6);
    if flags & 1 != 0 {
        return Err(CnpjError::Zip("encrypted entry".into()));
    }
    let method = le16(8);
    let compressed = u32::from_le_bytes([header[18], header[19], header[20], header[21]]);
    let skip = u64::from(le16(26)) + u64::from(le16(28));
    std::io::copy(&mut (&mut input).take(skip), &mut std::io::sink())
        .map_err(|e| CnpjError::Io(e.to_string()))?;
    match method {
        DEFLATED => rows(DeflateDecoder::new(input), &mut f),
        // A stored entry has no end marker: read only its size. Zip64 sizes are not read.
        STORED if compressed != u32::MAX => rows(input.take(u64::from(compressed)), &mut f),
        other => Err(CnpjError::Zip(format!("method {other} is not supported"))),
    }
}

fn rows<R: Read>(input: R, f: &mut impl FnMut(&[String])) -> Result<Counts, CnpjError> {
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(b';')
        .has_headers(false)
        .flexible(true)
        .from_reader(BufReader::with_capacity(1 << 20, input));
    let mut record = csv::ByteRecord::new();
    let mut fields: Vec<String> = Vec::new();
    let mut counts = Counts::default();
    loop {
        match reader.read_byte_record(&mut record) {
            Ok(true) => {
                fields.clear();
                fields.extend(record.iter().map(latin1));
                counts.rows += 1;
                f(&fields);
            }
            Ok(false) => return Ok(counts),
            Err(e) if e.is_io_error() => return Err(CnpjError::Io(e.to_string())),
            Err(_) => counts.rejected += 1,
        }
    }
}

fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|b| char::from(*b)).collect()
}

fn text(value: &str) -> Option<String> {
    let t = value.trim();
    (!t.is_empty()).then(|| t.to_owned())
}

/// Read a Receita date (`YYYYMMDD`). `0` and empty values give `None`.
#[must_use]
pub fn date(value: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(value.trim(), "%Y%m%d").ok()
}

/// One person in the file of partners ("Sócios").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Partner {
    /// The first 8 digits of the CNPJ.
    pub cnpj_basico: String,
    /// The name.
    pub name: String,
    /// The six middle digits of the CPF.
    pub cpf_part: String,
    /// The code of the qualification (role) of the partner.
    pub role: String,
    /// The date when the person became a partner.
    pub since: Option<NaiveDate>,
}

/// Read a row of the file of partners. Only people (identifier `2`) with a masked CPF give
/// a value. Columns: CNPJ base, identifier, name, CPF/CNPJ, qualification, entry date, ...
#[must_use]
pub fn partner(row: &[String]) -> Option<Partner> {
    if row.len() < 6 || row[1].trim() != "2" {
        return None;
    }
    let cpf_part: String = row[3].chars().filter(char::is_ascii_digit).collect();
    if cpf_part.len() != 6 {
        return None;
    }
    Some(Partner {
        cnpj_basico: row[0].trim().to_owned(),
        name: row[2].trim().to_owned(),
        cpf_part,
        role: row[4].trim().to_owned(),
        since: date(&row[5]),
    })
}

/// One company in the file of companies ("Empresas").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Company {
    /// The first 8 digits of the CNPJ.
    pub cnpj_basico: String,
    /// The legal name ("razão social").
    pub name: String,
    /// The code of the legal nature.
    pub legal_nature: String,
    /// The share capital in cents.
    pub capital_cents: Option<i64>,
    /// `micro`, `small`, `other` or `unknown`.
    pub size: &'static str,
}

/// Read a row of the file of companies. Columns: CNPJ base, name, legal nature,
/// qualification of the responsible person, share capital, size, federal entity.
#[must_use]
pub fn company(row: &[String]) -> Option<Company> {
    if row.len() < 6 {
        return None;
    }
    Some(Company {
        cnpj_basico: row[0].trim().to_owned(),
        name: row[1].trim().to_owned(),
        legal_nature: row[2].trim().to_owned(),
        capital_cents: cents(&row[4]),
        size: match row[5].trim() {
            "01" => "micro",
            "03" => "small",
            "05" => "other",
            _ => "unknown",
        },
    })
}

/// The head office ("matriz") of a company, from the file of establishments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadOffice {
    /// The first 8 digits of the CNPJ.
    pub cnpj_basico: String,
    /// The trade name.
    pub trade_name: Option<String>,
    /// `active`, `suspended`, `unfit`, `closed`, `null` or `unknown`.
    pub status: &'static str,
    /// The date when the activity started.
    pub opened_on: Option<NaiveDate>,
    /// The main CNAE code (7 digits).
    pub cnae: Option<String>,
    /// The state.
    pub uf: Option<String>,
    /// The code of the city (Receita table).
    pub city: Option<String>,
}

/// Read a row of the file of establishments. Only head offices (identifier `1`) give a
/// value. Columns used: 0 CNPJ base, 3 head office or branch, 4 trade name, 5 status,
/// 10 start date, 11 main CNAE, 19 state, 20 city.
#[must_use]
pub fn head_office(row: &[String]) -> Option<HeadOffice> {
    if row.len() < 21 || row[3].trim() != "1" {
        return None;
    }
    Some(HeadOffice {
        cnpj_basico: row[0].trim().to_owned(),
        trade_name: text(&row[4]),
        status: match row[5].trim() {
            "01" => "null",
            "02" => "active",
            "03" => "suspended",
            "04" => "unfit",
            "08" => "closed",
            _ => "unknown",
        },
        opened_on: date(&row[10]),
        cnae: text(&row[11]),
        uf: text(&row[19]),
        city: text(&row[20]),
    })
}

/// Read a row of a code table (CNAE, qualification, legal nature, city): code and text.
#[must_use]
pub fn code(row: &[String]) -> Option<(String, String)> {
    if row.len() < 2 {
        return None;
    }
    Some((row[0].trim().to_owned(), text(&row[1])?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_owned()).collect()
    }

    #[test]
    fn partner_keeps_people_with_a_masked_cpf() {
        let row = s(&[
            "07396865",
            "2",
            "GERSON HOFFMANN",
            "***240659**",
            "49",
            "20050518",
        ]);
        let p = partner(&row).unwrap();
        assert_eq!(p.cpf_part, "240659");
        assert_eq!(p.since, NaiveDate::from_ymd_opt(2005, 5, 18));
        assert!(partner(&s(&["1", "1", "HOLDING SA", "12345678000199", "22", "0"])).is_none());
        assert!(partner(&s(&["1", "2", "X", "***00**", "22", "0"])).is_none());
        assert!(partner(&s(&["1", "2"])).is_none());
    }

    #[test]
    fn company_reads_capital_and_size() {
        let row = s(&[
            "00000000",
            "BANCO DO BRASIL SA",
            "2038",
            "10",
            "120000000000,00",
            "05",
            "",
        ]);
        let c = company(&row).unwrap();
        assert_eq!(c.capital_cents, Some(12_000_000_000_000));
        assert_eq!(c.size, "other");
        let sizes: Vec<_> = ["01", "03", "00"]
            .iter()
            .map(|z| company(&s(&["1", "N", "1", "1", "x", z])).unwrap())
            .map(|c| (c.size, c.capital_cents))
            .collect();
        assert_eq!(sizes, [("micro", None), ("small", None), ("unknown", None)]);
        assert!(company(&s(&["1"])).is_none());
    }

    #[test]
    fn head_office_skips_branches() {
        let mut row = vec![String::new(); 30];
        row[0] = "07396865".into();
        row[3] = "1".into();
        row[5] = "08".into();
        row[10] = "20050518".into();
        row[11] = "1412602".into();
        row[19] = "SC".into();
        row[20] = "8297".into();
        let h = head_office(&row).unwrap();
        assert_eq!(
            (h.status, h.cnae.as_deref(), h.uf.as_deref()),
            ("closed", Some("1412602"), Some("SC"))
        );
        assert_eq!(h.trade_name, None);
        let statuses: Vec<_> = ["01", "02", "03", "04", "99"]
            .iter()
            .map(|st| {
                row[5] = (*st).into();
                head_office(&row).unwrap().status
            })
            .collect();
        assert_eq!(
            statuses,
            ["null", "active", "suspended", "unfit", "unknown"]
        );
        row[3] = "2".into();
        assert!(head_office(&row).is_none());
        assert!(head_office(&s(&["1"])).is_none());
    }

    #[test]
    fn code_and_date() {
        assert_eq!(
            code(&s(&["49", "Sócio-Administrador"])),
            Some(("49".into(), "Sócio-Administrador".into()))
        );
        assert_eq!(code(&s(&["49", " "])), None);
        assert_eq!(code(&s(&["49"])), None);
        assert_eq!(date("0"), None);
        assert_eq!(date(""), None);
    }
}
