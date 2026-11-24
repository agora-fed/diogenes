//! The shapes of the Câmara API responses. Only the fields that DIOGENES uses.
//! Every optional or nullable field is an `Option`, because the API sends `null` often.

use serde::Deserialize;

/// A deputy in a list (`/deputados`, `/frentes/{id}/membros`, votes).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeputySummary {
    /// The official id.
    pub id: i32,
    /// The parliamentary name.
    pub nome: String,
    /// The party acronym.
    #[serde(default)]
    pub sigla_partido: Option<String>,
    /// The state.
    #[serde(default)]
    pub sigla_uf: Option<String>,
    /// The photo URL.
    #[serde(default)]
    pub url_foto: Option<String>,
    /// The office e-mail.
    #[serde(default)]
    pub email: Option<String>,
    /// The role in a front (members list only).
    #[serde(default)]
    pub titulo: Option<String>,
}

/// The detail of a deputy (`/deputados/{id}`). `Debug` does not show the CPF.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeputyDetail {
    /// The official id.
    pub id: i32,
    /// The civil name.
    #[serde(default)]
    pub nome_civil: Option<String>,
    /// The CPF. DIOGENES uses it only to join with TSE data. It is not stored.
    #[serde(default)]
    pub cpf: Option<String>,
    /// "M" or "F".
    #[serde(default)]
    pub sexo: Option<String>,
    /// The last status.
    #[serde(default)]
    pub ultimo_status: Option<DeputyStatus>,
}

impl std::fmt::Debug for DeputyDetail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeputyDetail")
            .field("id", &self.id)
            .field("cpf", &self.cpf.as_ref().map(|_| "<redacted>"))
            .field("sexo", &self.sexo)
            .finish_non_exhaustive()
    }
}

/// The last status of a deputy.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeputyStatus {
    /// For example "Exercício" or "Vacância".
    #[serde(default)]
    pub situacao: Option<String>,
}

/// A party (`/partidos`).
#[derive(Debug, Clone, Deserialize)]
pub struct Party {
    /// The official id.
    pub id: i32,
    /// The acronym.
    pub sigla: String,
    /// The name.
    pub nome: String,
}

/// A leader record (`/legislaturas/{id}/lideres`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Leader {
    /// The deputy.
    pub parlamentar: DeputySummary,
    /// The title, for example "Líder" or "Vice-Líder".
    pub titulo: String,
    /// The bench.
    pub bancada: LeaderBench,
    /// The end date. `None` = the record is current.
    #[serde(default)]
    pub data_fim: Option<String>,
}

/// The bench of a leader.
#[derive(Debug, Clone, Deserialize)]
pub struct LeaderBench {
    /// "Partido Político", "Bloco Parlamentar", "Liderança", ...
    #[serde(default)]
    pub tipo: Option<String>,
    /// The name of the bench.
    pub nome: String,
}

/// A front in a list (`/frentes`).
#[derive(Debug, Clone, Deserialize)]
pub struct FrontSummary {
    /// The official id.
    pub id: i32,
    /// The title.
    pub titulo: String,
}

/// The detail of a front (`/frentes/{id}`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontDetail {
    /// The official id.
    pub id: i32,
    /// The title.
    pub titulo: String,
    /// The status text. It holds "Confirmadas N assinaturas de deputados".
    #[serde(default)]
    pub situacao: Option<String>,
    /// The phone.
    #[serde(default)]
    pub telefone: Option<String>,
    /// The URL of the founding document.
    #[serde(default)]
    pub url_documento: Option<String>,
    /// The coordinator. The API sometimes sends a name with `"id": null`.
    #[serde(default)]
    pub coordenador: Option<Coordinator>,
}

/// The coordinator of a front.
#[derive(Debug, Clone, Deserialize)]
pub struct Coordinator {
    /// The deputy id. `None` when the API does not know it.
    #[serde(default)]
    pub id: Option<i32>,
    /// The name.
    #[serde(default)]
    pub nome: Option<String>,
}

/// A roll call in a list (`/votacoes`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RollCallSummary {
    /// The official id, for example "2655570-60".
    pub id: String,
    /// The date (YYYY-MM-DD).
    pub data: String,
    /// The registration time.
    #[serde(default)]
    pub data_hora_registro: Option<String>,
    /// The body acronym, for example "PLEN".
    #[serde(default)]
    pub sigla_orgao: Option<String>,
    /// The description.
    #[serde(default)]
    pub descricao: Option<String>,
    /// 1 = approved, 0 = rejected, null = unknown.
    #[serde(default)]
    pub aprovacao: Option<i32>,
}

/// The detail of a roll call (`/votacoes/{id}`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RollCallDetail {
    /// The bills that the roll call affects.
    #[serde(default)]
    pub proposicoes_afetadas: Vec<BillSummary>,
}

/// A bill.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BillSummary {
    /// The official id.
    pub id: i32,
    /// The type acronym, for example "PL".
    pub sigla_tipo: String,
    /// The number.
    pub numero: i32,
    /// The year.
    pub ano: i32,
    /// The summary ("ementa").
    #[serde(default)]
    pub ementa: Option<String>,
}

/// One vote (`/votacoes/{id}/votos`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Vote {
    /// "Sim", "Não", "Abstenção", "Obstrução", "Artigo 17".
    pub tipo_voto: String,
    /// The deputy.
    #[serde(rename = "deputado_")]
    pub deputado: DeputySummary,
}

/// One orientation (`/votacoes/{id}/orientacoes`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Orientation {
    /// "Sim", "Não", "Liberado", ...
    pub orientacao_voto: String,
    /// "P" = party, "B" = bloc or leadership.
    #[serde(default)]
    pub cod_tipo_lideranca: Option<String>,
    /// The party or bloc name.
    pub sigla_partido_bloco: String,
}

/// A member of a body (`/orgaos/{id}/membros`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BodyMember {
    /// The deputy id.
    pub id: i32,
    /// The parliamentary name.
    pub nome: String,
    /// The role, for example "Presidente".
    #[serde(default)]
    pub titulo: Option<String>,
    /// The end date. `None` = current.
    #[serde(default)]
    pub data_fim: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::DeputyDetail;

    #[test]
    fn front_coordinator_with_null_id_is_accepted() {
        // Real response of /frentes/54531 (trimmed).
        let raw = r#"{"id": 54531, "titulo": "Frente Parlamentar Mista PROSUL",
            "coordenador": {"id": null, "uri": null, "nome": "Ismael", "siglaPartido": null}}"#;
        let front: super::FrontDetail = serde_json::from_str(raw).unwrap_or_else(|e| panic!("{e}"));
        let coordinator = front.coordenador.unwrap_or_else(|| unreachable!());
        assert_eq!(coordinator.id, None);
        assert_eq!(coordinator.nome.as_deref(), Some("Ismael"));
    }

    #[test]
    fn debug_never_shows_the_cpf() {
        let detail: DeputyDetail =
            serde_json::from_str(r#"{"id": 1, "cpf": "12345678901", "sexo": "F"}"#)
                .unwrap_or_else(|_| unreachable!());
        let text = format!("{detail:?}");
        assert!(!text.contains("12345678901"));
        assert!(text.contains("redacted"));
    }
}
