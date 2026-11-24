//! Name matching and party-orientation rules.
//!
//! These rules come from the original "Placar de Votações" design note:
//! * Match a deputy by the official id first. Then by the exact normalized name.
//!   Then by name containment in the same UF, only when the result is unique.
//! * In pasted text, scan for full names at word limits, the longest name first.
//!   Remove each match from the text, so that a short name does not match inside a
//!   long name ("Bacelar" inside "João Carlos Bacelar").
//! * Find the orientation of a party by exact acronym, then by the acronym as a
//!   word in a bloc name, then by prefix (truncated acronyms).

use std::collections::HashMap;

/// Minimum length of a name for a containment match.
const MIN_CONTAINMENT_LEN: usize = 5;
/// Minimum length of a bloc key for a prefix match.
const MIN_PREFIX_LEN: usize = 6;
/// Minimum length of a line for a unique-substring match in a pasted list.
const MIN_LINE_LEN: usize = 4;

/// Remove diacritics, set lower case and collapse white space.
#[must_use]
pub fn normalize(input: &str) -> String {
    let folded: String = input
        .chars()
        .map(fold_char)
        .collect::<String>()
        .to_lowercase();
    folded.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn fold_char(c: char) -> char {
    match c {
        'á' | 'à' | 'â' | 'ã' | 'ä' | 'å' => 'a',
        'Á' | 'À' | 'Â' | 'Ã' | 'Ä' | 'Å' => 'A',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'É' | 'È' | 'Ê' | 'Ë' => 'E',
        'í' | 'ì' | 'î' | 'ï' => 'i',
        'Í' | 'Ì' | 'Î' | 'Ï' => 'I',
        'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
        'Ó' | 'Ò' | 'Ô' | 'Õ' | 'Ö' => 'O',
        'ú' | 'ù' | 'û' | 'ü' => 'u',
        'Ú' | 'Ù' | 'Û' | 'Ü' => 'U',
        'ç' => 'c',
        'Ç' => 'C',
        'ñ' => 'n',
        'Ñ' => 'N',
        other => other,
    }
}

/// Replace each run of characters that are not `a-z` or `0-9` with one space.
fn words_only(normalized: &str) -> String {
    let mut out = String::with_capacity(normalized.len());
    let mut last_space = true;
    for c in normalized.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
            last_space = false;
        } else if !last_space {
            out.push(' ');
            last_space = true;
        }
    }
    out.trim_end().to_owned()
}

/// One deputy in the index.
#[derive(Debug, Clone)]
struct Entry {
    name: String,
    camara_id: i32,
    uf: String,
}

/// An index of deputies for name matching.
#[derive(Debug, Clone, Default)]
pub struct DeputyIndex {
    known_ids: std::collections::HashSet<i32>,
    exact: HashMap<String, i32>,
    entries: Vec<Entry>,
    /// Entries sorted by name length, the longest first (for the scan).
    longest_first: Vec<usize>,
}

impl DeputyIndex {
    /// Make an index from (camara id, name, UF) triples.
    #[must_use]
    pub fn new<'a>(deputies: impl IntoIterator<Item = (i32, &'a str, &'a str)>) -> Self {
        let mut index = Self::default();
        for (camara_id, name, uf) in deputies {
            let name = normalize(name);
            index.known_ids.insert(camara_id);
            index.exact.entry(name.clone()).or_insert(camara_id);
            index.entries.push(Entry {
                name,
                camara_id,
                uf: uf.to_uppercase(),
            });
        }
        let mut order: Vec<usize> = (0..index.entries.len()).collect();
        order.sort_by_key(|i| std::cmp::Reverse(index.entries[*i].name.len()));
        index.longest_first = order;
        index
    }

    /// Find one deputy. See the module notes for the order of the rules.
    #[must_use]
    pub fn find(&self, camara_id: Option<i32>, name: &str, uf: Option<&str>) -> Option<i32> {
        if let Some(id) = camara_id.filter(|id| self.known_ids.contains(id)) {
            return Some(id);
        }
        let wanted = normalize(name);
        if wanted.is_empty() {
            return None;
        }
        if let Some(id) = self.exact.get(&wanted) {
            return Some(*id);
        }
        let uf = uf.map(str::to_uppercase).filter(|u| !u.is_empty());
        let mut hits = self.entries.iter().filter(|e| {
            uf.as_deref().is_none_or(|u| e.uf == u)
                && e.name.len().min(wanted.len()) >= MIN_CONTAINMENT_LEN
                && (e.name.contains(&wanted) || wanted.contains(&e.name))
        });
        let first = hits.next()?;
        if hits.next().is_some() {
            return None;
        }
        Some(first.camara_id)
    }

    /// Find all full names in a free text. The longest names match first, and each
    /// match is removed from the text.
    #[must_use]
    pub fn scan(&self, text: &str) -> Vec<i32> {
        let mut haystack = format!(" {} ", words_only(&normalize(text)));
        let mut found = Vec::new();
        for i in &self.longest_first {
            let entry = &self.entries[*i];
            let needle = format!(" {} ", words_only(&entry.name));
            if needle.trim().is_empty() || !haystack.contains(&needle) {
                continue;
            }
            found.push(entry.camara_id);
            haystack = haystack.replace(&needle, "  ");
        }
        found
    }

    /// Match a pasted list (one name per line, or names split by `;` or `,`).
    /// Return the matched ids and the lines with no match.
    #[must_use]
    pub fn match_list(&self, text: &str) -> ListMatch {
        let mut result = ListMatch::default();
        for raw in text.split(['\n', ';', ',']) {
            let full = normalize(raw);
            if full.is_empty() {
                continue;
            }
            // Try the full line first: a name can start with a title word.
            let line = strip_title(&full);
            if let Some(id) = self.match_line(&full).or_else(|| self.match_line(&line)) {
                result.found.push(id);
                continue;
            }
            let scanned = self.scan(&line);
            if scanned.is_empty() {
                result.missed.push(raw.trim().to_owned());
            } else {
                result.found.extend(scanned);
            }
        }
        result.found.dedup();
        result
    }

    fn match_line(&self, line: &str) -> Option<i32> {
        let best = self
            .entries
            .iter()
            .filter(|e| line == e.name || line.starts_with(&format!("{} ", e.name)))
            .max_by_key(|e| e.name.len());
        if let Some(entry) = best {
            return Some(entry.camara_id);
        }
        if line.len() < MIN_LINE_LEN {
            return None;
        }
        let mut hits = self.entries.iter().filter(|e| e.name.contains(line));
        let first = hits.next()?;
        hits.next().is_none().then_some(first.camara_id)
    }
}

fn strip_title(line: &str) -> String {
    for prefix in ["deputada ", "deputado ", "dep. ", "dep "] {
        if let Some(rest) = line.strip_prefix(prefix) {
            return rest.trim().to_owned();
        }
    }
    line.to_owned()
}

/// The result of [`DeputyIndex::match_list`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListMatch {
    /// The matched Câmara ids, in input order.
    pub found: Vec<i32>,
    /// The input lines with no match.
    pub missed: Vec<String>,
}

/// A party or bloc orientation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    /// The leader recommends "Sim".
    Yes,
    /// The leader recommends "Não".
    No,
    /// Any other orientation ("Liberado", "Obstrução", ...).
    Other,
}

/// Find the orientation that applies to a party. `orientations` holds (bloc, orientation).
#[must_use]
pub fn orientation_for(party: &str, orientations: &[(String, String)]) -> Option<Orientation> {
    let party = normalize(party);
    let keyed: Vec<(String, &str)> = orientations
        .iter()
        .map(|(bloc, o)| (normalize(bloc), o.as_str()))
        .collect();
    let hit = keyed
        .iter()
        .find(|(bloc, _)| *bloc == party)
        .or_else(|| {
            keyed.iter().find(|(bloc, _)| {
                bloc.split(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit()))
                    .any(|w| w == party)
            })
        })
        .or_else(|| {
            keyed
                .iter()
                .find(|(bloc, _)| bloc.len() >= MIN_PREFIX_LEN && party.starts_with(bloc.as_str()))
        })?;
    Some(match normalize(hit.1).as_str() {
        "sim" => Orientation::Yes,
        "nao" => Orientation::No,
        _ => Orientation::Other,
    })
}

/// The kind of an individual vote in a roll call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoteKind {
    /// "Sim".
    Yes,
    /// "Não".
    No,
    /// "Abstenção".
    Abstention,
    /// "Obstrução".
    Obstruction,
    /// "Artigo 17": the chair does not vote.
    Chair,
    /// Any other value.
    Other,
}

impl VoteKind {
    /// Parse the `tipoVoto` value of the Câmara API.
    #[must_use]
    pub fn parse(raw: &str) -> Self {
        match normalize(raw).as_str() {
            "sim" => Self::Yes,
            "nao" => Self::No,
            "abstencao" => Self::Abstention,
            "obstrucao" => Self::Obstruction,
            s if s.starts_with("art") => Self::Chair,
            _ => Self::Other,
        }
    }
}

/// The totals in a roll-call description, for example "Sim: 319; Não: 127; Abstenção: 1".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DescribedTotals {
    /// "Sim" total.
    pub yes: Option<i64>,
    /// "Não" total.
    pub no: Option<i64>,
    /// "Abstenção" total.
    pub abstention: Option<i64>,
}

impl DescribedTotals {
    /// Read the totals from a description. Return `None` when it has no totals.
    #[must_use]
    pub fn parse(description: &str) -> Option<Self> {
        let text = normalize(description);
        let totals = Self {
            yes: number_after(&text, "sim:"),
            no: number_after(&text, "nao:"),
            abstention: number_after(&text, "abstencao:"),
        };
        (totals.yes.is_some() || totals.no.is_some()).then_some(totals)
    }
}

fn number_after(text: &str, label: &str) -> Option<i64> {
    let start = text.find(label)? + label.len();
    let digits: String = text[start..]
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// The signature record of a front, read from its status text.
///
/// The status text is a record of events: the signatures at creation, then additions
/// ("Confirmadas 21 assinaturas de Deputados", "Confirmada adesão de um Deputado") and
/// removals ("Confirmada a retirada"). The members list of the API is a different thing:
/// it drops deputies who left office. So these numbers are information, not a check.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrontSignatures {
    /// Deputy signatures at creation (the first confirmation).
    pub at_creation: Option<i64>,
    /// Signatures at creation, plus additions, minus removals.
    pub net: Option<i64>,
}

/// Read the signature record of a front. See [`FrontSignatures`].
#[must_use]
pub fn front_signatures(status: &str) -> FrontSignatures {
    let mut out = FrontSignatures::default();
    // Split into lines first: `normalize` joins all white space, line breaks too.
    for line in status.lines().map(normalize) {
        let delta = if let Some(n) = confirmed_deputies(&line) {
            Some(n)
        } else if line.contains("confirmada adesao de um deputad") {
            Some(1)
        } else if line.contains("confirmada a retirada") {
            Some(-1)
        } else {
            None
        };
        let Some(delta) = delta else { continue };
        if out.at_creation.is_none() && delta > 0 {
            out.at_creation = Some(delta);
        }
        out.net = Some(out.net.unwrap_or(0) + delta);
    }
    out
}

/// "confirmadas N assinaturas de deputados" in one line: return N.
fn confirmed_deputies(line: &str) -> Option<i64> {
    let pos = line.find("confirmadas ")? + "confirmadas ".len();
    let rest = &line[pos..];
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    rest[digits.len()..]
        .trim_start()
        .starts_with("assinaturas de deputad")
        .then(|| digits.parse().ok())
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> DeputyIndex {
        DeputyIndex::new([
            (1, "João Carlos Bacelar", "BA"),
            (2, "Bacelar", "BA"),
            (3, "Érika Hilton", "SP"),
            (4, "Pedro Lucas Fernandes", "MA"),
            (5, "Pedro Uczai", "SC"),
        ])
    }

    #[test]
    fn normalize_removes_accents_and_spaces() {
        assert_eq!(normalize("  Érika   HILTON "), "erika hilton");
        assert_eq!(normalize("Conceição"), "conceicao");
    }

    #[test]
    fn find_prefers_official_id() {
        assert_eq!(index().find(Some(3), "anything", None), Some(3));
    }

    #[test]
    fn find_ignores_unknown_id_and_uses_name() {
        assert_eq!(index().find(Some(999), "erika hilton", None), Some(3));
    }

    #[test]
    fn find_uses_unique_containment_in_same_uf() {
        assert_eq!(index().find(None, "Pedro Lucas", Some("MA")), Some(4));
    }

    #[test]
    fn find_rejects_ambiguous_containment() {
        // "pedro" is in two names: the match is not unique.
        assert_eq!(index().find(None, "Pedro", None), None);
    }

    #[test]
    fn find_returns_none_for_empty_name() {
        assert_eq!(index().find(None, "   ", None), None);
    }

    #[test]
    fn scan_does_not_match_short_name_inside_long_name() {
        let ids = index().scan("Votaram: João Carlos Bacelar (PL/BA) e Érika Hilton.");
        assert_eq!(ids, vec![1, 3]);
    }

    #[test]
    fn scan_finds_short_name_when_it_stands_alone() {
        assert_eq!(index().scan("Bacelar"), vec![2]);
    }

    #[test]
    fn match_list_reports_misses() {
        let result = index().match_list("Dep. Érika Hilton\nFulano de Tal; Pedro Uczai (PT/SC)");
        assert_eq!(result.found, vec![3, 5]);
        assert_eq!(result.missed, vec!["Fulano de Tal".to_owned()]);
    }

    #[test]
    fn match_list_keeps_names_that_start_with_a_title() {
        let idx = DeputyIndex::new([(7, "Delegado Palumbo", "SP"), (8, "Deputado Teste", "RJ")]);
        assert_eq!(
            idx.match_list("Deputado Teste\nDep. Delegado Palumbo")
                .found,
            vec![8, 7]
        );
    }

    #[test]
    fn orientation_exact_word_and_prefix() {
        let o = vec![
            ("PT".to_owned(), "Sim".to_owned()),
            ("Fdr PSOL-REDE".to_owned(), "Não".to_owned()),
            ("Republicanos".to_owned(), "Liberado".to_owned()),
        ];
        assert_eq!(orientation_for("PT", &o), Some(Orientation::Yes));
        assert_eq!(orientation_for("PSOL", &o), Some(Orientation::No));
        assert_eq!(
            orientation_for("REPUBLICANOS", &o),
            Some(Orientation::Other)
        );
        assert_eq!(orientation_for("NOVO", &o), None);
    }

    #[test]
    fn vote_kind_parses_api_values() {
        assert_eq!(VoteKind::parse("Sim"), VoteKind::Yes);
        assert_eq!(VoteKind::parse("Não"), VoteKind::No);
        assert_eq!(VoteKind::parse("Abstenção"), VoteKind::Abstention);
        assert_eq!(VoteKind::parse("Obstrução"), VoteKind::Obstruction);
        assert_eq!(VoteKind::parse("Artigo 17"), VoteKind::Chair);
        assert_eq!(VoteKind::parse("?"), VoteKind::Other);
    }

    #[test]
    fn described_totals_are_parsed() {
        let t = DescribedTotals::parse(
            "Mantido o texto. Sim: 319; Não: 127; Abstenção: 1; Total: 447.",
        );
        assert_eq!(
            t,
            Some(DescribedTotals {
                yes: Some(319),
                no: Some(127),
                abstention: Some(1)
            })
        );
        assert_eq!(DescribedTotals::parse("Aprovada a Redação Final."), None);
    }

    #[test]
    fn front_signatures_read_the_record() {
        let s = "08/03/2023 - Confirmadas 300 assinaturas de deputados. Há senadores.\n\
                 25/09/2023 - Confirmadas 21 assinaturas de Deputados. Despacho.\n\
                 16/11/2023 - Confirmada a retirada. Despacho.\n\
                 29/11/2024 - Confirmada adesão de um Deputado. Despacho.\n\
                 04/05/2026 - Confirmadas 4 assinaturas de deputados e 4 senadores.";
        assert_eq!(
            front_signatures(s),
            FrontSignatures {
                at_creation: Some(300),
                net: Some(325)
            }
        );
        assert_eq!(front_signatures("sem dados"), FrontSignatures::default());
    }
}
