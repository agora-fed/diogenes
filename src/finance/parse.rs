//! Read the TSE campaign-finance files ("prestação de contas eleitorais").
//!
//! The archive has one CSV per state and one national CSV. DIOGENES reads two national
//! files: the receipts of the candidates, and the original donors of money that came
//! through a party. The files are latin-1 with `;` separators. The reader streams the
//! rows: the receipt file is larger than 400 MB.

use std::collections::HashMap;
use std::io::Read;

use crate::tse::TseError;

/// The national receipt file in the archive.
pub const RECEIPTS_FILE: &str = "receitas_candidatos_2022_BRASIL.csv";
/// The national original-donor file in the archive.
pub const ORIGINAL_DONORS_FILE: &str = "receitas_candidatos_doador_originario_2022_BRASIL.csv";
const OFFICE: &str = "DEPUTADO FEDERAL";
/// The TSE marks an empty value with this text.
const NULL: &str = "#NULO";

/// Where the money of a receipt comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Source {
    /// Donations of people.
    Individuals,
    /// Transfers of a party (public funds and party money).
    Party,
    /// Money of the candidate.
    Own,
    /// Transfers of other candidates.
    Candidates,
    /// Crowdfunding platforms.
    Crowdfunding,
    /// All other origins (interest, unidentified, sales).
    Other,
}

impl Source {
    /// The value in the database.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Individuals => "individuals",
            Self::Party => "party",
            Self::Own => "own",
            Self::Candidates => "candidates",
            Self::Crowdfunding => "crowdfunding",
            Self::Other => "other",
        }
    }

    /// Read the `DS_ORIGEM_RECEITA` text.
    #[must_use]
    pub fn parse(origin: &str) -> Self {
        let o = origin.to_lowercase();
        if o.starts_with("recursos de pessoas f") || o.starts_with("doações pela internet") {
            Self::Individuals
        } else if o.starts_with("recursos de partido") {
            Self::Party
        } else if o.starts_with("recursos próprios") {
            Self::Own
        } else if o.starts_with("recursos de outros candidatos") {
            Self::Candidates
        } else if o.contains("financiamento coletivo") {
            Self::Crowdfunding
        } else {
            Self::Other
        }
    }
}

/// The kind of a donor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DonorKind {
    /// A person (CPF).
    Person,
    /// The campaign of another candidate.
    Candidate,
}

impl DonorKind {
    /// The value in the database.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Person => "person",
            Self::Candidate => "candidate",
        }
    }
}

/// A donor. `doc` is the CPF or CNPJ (digits). It stays in memory only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Donor {
    /// The CPF or CNPJ, digits only.
    pub doc: String,
    /// The kind.
    pub kind: DonorKind,
    /// The name as the TSE publishes it.
    pub name: String,
    /// The economic activity (CNAE), when the TSE gives one.
    pub activity: Option<String>,
    /// The state of the donor.
    pub uf: Option<String>,
}

/// One receipt of a deputy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    /// The Câmara id of the deputy.
    pub camara_id: i32,
    /// The TSE receipt number (`SQ_RECEITA`). The original-donor file refers to it.
    pub seq: String,
    /// The origin of the money.
    pub source: Source,
    /// The donor, for donations of people and of other candidates.
    pub donor: Option<Donor>,
    /// The value in cents.
    pub cents: i64,
}

/// One original donor of money that came to a deputy through a party.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OriginalDonation {
    /// The Câmara id of the deputy.
    pub camara_id: i32,
    /// The original donor.
    pub donor: Donor,
    /// The value in cents.
    pub cents: i64,
}

/// The rows that the reader could not use. The import shows them as checks.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Rejected {
    /// Rows with a value that is not a number.
    pub bad_value: i64,
    /// Rows that the CSV reader refused.
    pub bad_row: i64,
}

/// Parse a TSE value ("1.050,25") into cents.
#[must_use]
pub fn cents(value: &str) -> Option<i64> {
    let v = value.trim().replace('.', "");
    let (int, frac) = v.split_once(',').unwrap_or((&v, "0"));
    let int: i64 = int.parse().ok()?;
    let frac = format!("{frac:0<2}");
    let frac: i64 = frac.get(..2)?.parse().ok()?;
    if int < 0 {
        return None;
    }
    Some(int * 100 + frac)
}

fn digits(s: &str) -> String {
    s.chars().filter(char::is_ascii_digit).collect()
}

fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|b| char::from(*b)).collect()
}

fn value(s: String) -> Option<String> {
    let t = s.trim();
    (!t.is_empty() && t != NULL && t != "-1").then(|| t.to_owned())
}

/// A streaming reader of one TSE CSV with a column map.
struct Table<R: Read> {
    reader: csv::Reader<R>,
    cols: HashMap<String, usize>,
}

impl<R: Read> Table<R> {
    fn open(input: R, required: &[&str]) -> Result<Self, TseError> {
        let mut reader = csv::ReaderBuilder::new()
            .delimiter(b';')
            .flexible(true)
            .from_reader(input);
        let headers = reader
            .byte_headers()
            .map_err(|e| TseError::Csv(e.to_string()))?;
        let cols: HashMap<String, usize> = headers
            .iter()
            .enumerate()
            .map(|(i, h)| (latin1(h).trim().to_owned(), i))
            .collect();
        for name in required {
            if !cols.contains_key(*name) {
                return Err(TseError::Csv(format!("missing column {name}")));
            }
        }
        Ok(Self { reader, cols })
    }

    /// Call `f` for each row. Count the rows that the CSV reader refuses.
    fn each(mut self, rejected: &mut Rejected, mut f: impl FnMut(&dyn Fn(&str) -> String)) {
        let mut record = csv::ByteRecord::new();
        loop {
            match self.reader.read_byte_record(&mut record) {
                Ok(true) => {
                    let get = |name: &str| {
                        self.cols
                            .get(name)
                            .and_then(|i| record.get(*i))
                            .map(latin1)
                            .unwrap_or_default()
                    };
                    f(&get);
                }
                Ok(false) => break,
                Err(_) => rejected.bad_row += 1,
            }
        }
    }
}

const RECEIPT_COLUMNS: &[&str] = &[
    "DS_CARGO",
    "NR_CPF_CANDIDATO",
    "SQ_RECEITA",
    "DS_ORIGEM_RECEITA",
    "NR_CPF_CNPJ_DOADOR",
    "NM_DOADOR",
    "DS_CNAE_DOADOR",
    "SG_UF_DOADOR",
    "VR_RECEITA",
];

/// Read the receipts of the deputies in `cpfs` (CPF → Câmara id).
///
/// # Errors
/// Returns an error when a required column is missing.
pub fn receipts<R: Read>(
    input: R,
    cpfs: &HashMap<String, i32>,
    rejected: &mut Rejected,
) -> Result<Vec<Receipt>, TseError> {
    let table = Table::open(input, RECEIPT_COLUMNS)?;
    let mut out = Vec::new();
    let mut bad_value = 0;
    table.each(rejected, |get| {
        if !get("DS_CARGO").trim().eq_ignore_ascii_case(OFFICE) {
            return;
        }
        let Some(&camara_id) = cpfs.get(&digits(&get("NR_CPF_CANDIDATO"))) else {
            return;
        };
        let Some(cents) = cents(&get("VR_RECEITA")) else {
            bad_value += 1;
            return;
        };
        let source = Source::parse(&get("DS_ORIGEM_RECEITA"));
        let kind = match source {
            Source::Individuals => Some(DonorKind::Person),
            Source::Candidates => Some(DonorKind::Candidate),
            _ => None,
        };
        let doc = digits(&get("NR_CPF_CNPJ_DOADOR"));
        let donor = kind.filter(|_| doc.len() >= 11).map(|kind| Donor {
            doc,
            kind,
            name: value(get("NM_DOADOR")).unwrap_or_else(|| "Sem nome".into()),
            activity: value(get("DS_CNAE_DOADOR")),
            uf: value(get("SG_UF_DOADOR")),
        });
        out.push(Receipt {
            camara_id,
            seq: get("SQ_RECEITA").trim().to_owned(),
            source,
            donor,
            cents,
        });
    });
    rejected.bad_value += bad_value;
    Ok(out)
}

const ORIGINAL_COLUMNS: &[&str] = &[
    "SQ_RECEITA",
    "NR_CPF_CNPJ_DOADOR_ORIGINARIO",
    "NM_DOADOR_ORIGINARIO",
    "TP_DOADOR_ORIGINARIO",
    "DS_CNAE_DOADOR_ORIGINARIO",
    "VR_RECEITA",
];

/// Read the original donors of the party receipts in `party_receipts` (receipt → Câmara id).
/// Only people (`TP_DOADOR_ORIGINARIO` = `F`) are kept: public funds have no original donor.
///
/// # Errors
/// Returns an error when a required column is missing.
pub fn original_donors<R: Read>(
    input: R,
    party_receipts: &HashMap<String, i32>,
    rejected: &mut Rejected,
) -> Result<Vec<OriginalDonation>, TseError> {
    let table = Table::open(input, ORIGINAL_COLUMNS)?;
    let mut out = Vec::new();
    let mut bad_value = 0;
    table.each(rejected, |get| {
        let Some(&camara_id) = party_receipts.get(get("SQ_RECEITA").trim()) else {
            return;
        };
        if get("TP_DOADOR_ORIGINARIO").trim() != "F" {
            return;
        }
        let doc = digits(&get("NR_CPF_CNPJ_DOADOR_ORIGINARIO"));
        if doc.len() < 11 {
            return;
        }
        let Some(cents) = cents(&get("VR_RECEITA")) else {
            bad_value += 1;
            return;
        };
        out.push(OriginalDonation {
            camara_id,
            donor: Donor {
                doc,
                kind: DonorKind::Person,
                name: value(get("NM_DOADOR_ORIGINARIO")).unwrap_or_else(|| "Sem nome".into()),
                activity: value(get("DS_CNAE_DOADOR_ORIGINARIO")),
                uf: None,
            },
            cents,
        });
    });
    rejected.bad_value += bad_value;
    Ok(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    pub(crate) const RECEIPTS: &str = "\"DS_CARGO\";\"NR_CPF_CANDIDATO\";\"SQ_RECEITA\";\"DS_ORIGEM_RECEITA\";\"NR_CPF_CNPJ_DOADOR\";\"NM_DOADOR\";\"DS_CNAE_DOADOR\";\"SG_UF_DOADOR\";\"VR_RECEITA\"\n\
\"Deputado Federal\";\"111.111.111-11\";\"1\";\"Recursos de pessoas físicas\";\"22222222222\";\"JOSÉ DA SILVA\";\"#NULO\";\"SP\";\"1050,25\"\n\
\"Deputado Federal\";\"11111111111\";\"2\";\"Recursos de partido político\";\"12345678000199\";\"DIREÇÃO NACIONAL\";\"#NULO\";\"#NULO\";\"300000,00\"\n\
\"Deputado Federal\";\"11111111111\";\"3\";\"Recursos próprios\";\"11111111111\";\"O PRÓPRIO\";\"#NULO\";\"SP\";\"10,5\"\n\
\"Deputado Federal\";\"11111111111\";\"4\";\"Recursos de outros candidatos\";\"98765432000100\";\"ELEICAO 2022 FULANO\";\"#NULO\";\"RJ\";\"500,00\"\n\
\"Deputado Federal\";\"11111111111\";\"5\";\"Recursos de Financiamento Coletivo\";\"11222333000144\";\"PLATAFORMA\";\"#NULO\";\"#NULO\";\"20,00\"\n\
\"Deputado Federal\";\"11111111111\";\"6\";\"Rendimentos de aplicações financeiras\";\"#NULO\";\"#NULO\";\"#NULO\";\"#NULO\";\"1,00\"\n\
\"Deputado Federal\";\"11111111111\";\"7\";\"Recursos de pessoas físicas\";\"33333333333\";\"MARIA\";\"#NULO\";\"SP\";\"abc\"\n\
\"Deputado Estadual\";\"11111111111\";\"8\";\"Recursos de pessoas físicas\";\"22222222222\";\"JOSÉ\";\"#NULO\";\"SP\";\"9,00\"\n\
\"Deputado Federal\";\"99999999999\";\"9\";\"Recursos de pessoas físicas\";\"22222222222\";\"JOSÉ\";\"#NULO\";\"SP\";\"9,00\"\n";

    /// The TSE files are latin-1: encode the test text the same way.
    fn l1(s: &str) -> Vec<u8> {
        s.chars()
            .map(|c| u8::try_from(u32::from(c)).unwrap())
            .collect()
    }

    fn cpfs() -> HashMap<String, i32> {
        HashMap::from([("11111111111".to_owned(), 7)])
    }

    #[test]
    fn cents_reads_the_tse_format() {
        assert_eq!(cents("1050,25"), Some(105_025));
        assert_eq!(cents("1.050,2"), Some(105_020));
        assert_eq!(cents("7"), Some(700));
        assert_eq!(cents("0,00"), Some(0));
        assert_eq!(cents("-5,00"), None);
        assert_eq!(cents("abc"), None);
    }

    #[test]
    fn source_reads_the_origin_text() {
        assert_eq!(
            Source::parse("Recursos de pessoas físicas"),
            Source::Individuals
        );
        assert_eq!(Source::parse("Doações pela Internet"), Source::Individuals);
        assert_eq!(Source::parse("Recursos de partido político"), Source::Party);
        assert_eq!(Source::parse("Recursos próprios"), Source::Own);
        assert_eq!(
            Source::parse("Recursos de outros candidatos"),
            Source::Candidates
        );
        assert_eq!(
            Source::parse("Recursos de Financiamento Coletivo"),
            Source::Crowdfunding
        );
        assert_eq!(Source::parse("#NULO"), Source::Other);
        let keys: Vec<&str> = [
            Source::Individuals,
            Source::Party,
            Source::Own,
            Source::Candidates,
            Source::Crowdfunding,
            Source::Other,
        ]
        .iter()
        .map(|s| s.key())
        .collect();
        assert_eq!(
            keys,
            [
                "individuals",
                "party",
                "own",
                "candidates",
                "crowdfunding",
                "other"
            ]
        );
        assert_eq!(DonorKind::Person.key(), "person");
        assert_eq!(DonorKind::Candidate.key(), "candidate");
    }

    #[test]
    fn receipts_keep_only_known_federal_deputies() {
        let mut rejected = Rejected::default();
        let rows = receipts(l1(RECEIPTS).as_slice(), &cpfs(), &mut rejected).unwrap();
        assert_eq!(rows.len(), 6, "office, unknown CPF and bad value are out");
        assert_eq!(rejected.bad_value, 1);
        assert!(rows.iter().all(|r| r.camara_id == 7));
        let first = &rows[0];
        assert_eq!(first.cents, 105_025);
        assert_eq!(first.seq, "1");
        let donor = first.donor.as_ref().unwrap();
        assert_eq!(
            (
                donor.doc.as_str(),
                donor.kind,
                donor.name.as_str(),
                donor.activity.as_deref(),
                donor.uf.as_deref()
            ),
            (
                "22222222222",
                DonorKind::Person,
                "JOSÉ DA SILVA",
                None,
                Some("SP")
            )
        );
        let party = rows.iter().find(|r| r.source == Source::Party).unwrap();
        assert!(party.donor.is_none(), "a party is not a donor row");
        let other = rows
            .iter()
            .find(|r| r.source == Source::Candidates)
            .unwrap();
        assert_eq!(other.donor.as_ref().unwrap().kind, DonorKind::Candidate);
        assert!(rows
            .iter()
            .find(|r| r.source == Source::Own)
            .unwrap()
            .donor
            .is_none());
        assert!(rows
            .iter()
            .find(|r| r.source == Source::Crowdfunding)
            .unwrap()
            .donor
            .is_none());
        assert_eq!(
            rows.iter()
                .find(|r| r.source == Source::Other)
                .unwrap()
                .cents,
            100
        );
    }

    #[test]
    fn latin1_names_are_decoded() {
        let mut csv = b"DS_CARGO;NR_CPF_CANDIDATO;SQ_RECEITA;DS_ORIGEM_RECEITA;NR_CPF_CNPJ_DOADOR;NM_DOADOR;DS_CNAE_DOADOR;SG_UF_DOADOR;VR_RECEITA\n".to_vec();
        csv.extend_from_slice(b"DEPUTADO FEDERAL;11111111111;1;Recursos de pessoas f\xedsicas;22222222222;JO\xc3O;#NULO;SP;1,00\n");
        let rows = receipts(csv.as_slice(), &cpfs(), &mut Rejected::default()).unwrap();
        assert_eq!(rows[0].donor.as_ref().unwrap().name, "JOÃO");
        assert_eq!(rows[0].source, Source::Individuals);
    }

    #[test]
    fn missing_column_is_an_error() {
        let err = receipts(
            &b"DS_CARGO;VR_RECEITA\n"[..],
            &cpfs(),
            &mut Rejected::default(),
        );
        assert!(matches!(err, Err(TseError::Csv(m)) if m.contains("NR_CPF_CANDIDATO")));
        let err = original_donors(
            &b"SQ_RECEITA\n"[..],
            &HashMap::new(),
            &mut Rejected::default(),
        );
        assert!(matches!(err, Err(TseError::Csv(_))));
    }

    #[test]
    fn original_donors_follow_party_receipts() {
        let csv = "SQ_RECEITA;NR_CPF_CNPJ_DOADOR_ORIGINARIO;NM_DOADOR_ORIGINARIO;TP_DOADOR_ORIGINARIO;DS_CNAE_DOADOR_ORIGINARIO;VR_RECEITA\n\
2;44444444444;EMPRESÁRIO;F;#NULO;200000,00\n\
2;-1;#NULO;#NULO;#NULO;-1\n\
2;55555555555;OUTRO;F;#NULO;x\n\
3;66666666666;FORA;F;#NULO;10,00\n";
        let mut rejected = Rejected::default();
        let map = HashMap::from([("2".to_owned(), 7)]);
        let rows = original_donors(l1(csv).as_slice(), &map, &mut rejected).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].camara_id, 7);
        assert_eq!(rows[0].cents, 20_000_000);
        assert_eq!(rows[0].donor.name, "EMPRESÁRIO");
        assert_eq!(rejected.bad_value, 1);
    }

    #[test]
    fn a_broken_row_is_counted() {
        // An open quote that never closes makes the CSV reader fail on the row.
        let csv = b"DS_CARGO;NR_CPF_CANDIDATO;SQ_RECEITA;DS_ORIGEM_RECEITA;NR_CPF_CNPJ_DOADOR;NM_DOADOR;DS_CNAE_DOADOR;SG_UF_DOADOR;VR_RECEITA\n\
DEPUTADO FEDERAL;11111111111;1;Recursos de pessoas f;2;J;#NULO;SP;1,00;extra\n";
        let mut rejected = Rejected::default();
        let rows = receipts(&csv[..], &cpfs(), &mut rejected).unwrap();
        assert_eq!(
            rows.len(),
            1,
            "a flexible row with an extra field is still read"
        );
        assert!(
            rows[0].donor.is_none(),
            "a document shorter than a CPF is not a donor"
        );
    }
}
