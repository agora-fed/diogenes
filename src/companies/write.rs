//! Write the rows of the `companies` job in one transaction.

use std::collections::HashSet;

use chrono::NaiveDate;
use sqlx::PgPool;

use super::{Found, Target, DATASET};
use crate::finance::ELECTION_YEAR;

/// One company row, with the code texts.
#[derive(Debug, Default)]
struct CompanyCols {
    cnpj: Vec<String>,
    name: Vec<String>,
    trade_name: Vec<Option<String>>,
    nature: Vec<Option<String>>,
    capital: Vec<Option<i64>>,
    size: Vec<&'static str>,
    cnae: Vec<Option<String>>,
    cnae_text: Vec<Option<String>>,
    status: Vec<&'static str>,
    uf: Vec<Option<String>>,
    city: Vec<Option<String>>,
    opened: Vec<Option<NaiveDate>>,
}

fn company_cols(found: &Found) -> CompanyCols {
    let mut c = CompanyCols::default();
    let mut cnpjs: Vec<&String> = found.companies.keys().collect();
    cnpjs.sort();
    for cnpj in cnpjs {
        let co = &found.companies[cnpj];
        let office = found.offices.get(cnpj);
        let cnae = office.and_then(|o| o.cnae.clone());
        c.cnpj.push(cnpj.clone());
        c.name.push(co.name.clone());
        c.trade_name.push(office.and_then(|o| o.trade_name.clone()));
        c.nature.push(
            found
                .natures
                .get(&co.legal_nature)
                .cloned()
                .or_else(|| (!co.legal_nature.is_empty()).then(|| co.legal_nature.clone())),
        );
        c.capital.push(co.capital_cents);
        c.size.push(co.size);
        c.cnae_text
            .push(cnae.as_ref().and_then(|k| found.cnaes.get(k).cloned()));
        c.cnae.push(cnae);
        c.status.push(office.map_or("unknown", |o| o.status));
        c.uf.push(office.and_then(|o| o.uf.clone()));
        c.city.push(
            office
                .and_then(|o| o.city.as_ref())
                .and_then(|k| found.cities.get(k).cloned()),
        );
        c.opened.push(office.and_then(|o| o.opened_on));
    }
    c
}

/// Replace the companies, the links and the data-set version.
///
/// # Errors
/// Returns a database error. Then nothing changes.
pub async fn write(db: &PgPool, found: &Found, month: &str, url: &str) -> Result<(), sqlx::Error> {
    let c = company_cols(found);
    let mut tx = db.begin().await?;
    sqlx::query("DELETE FROM diogenes_donor_company")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM diogenes_deputy_company")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM diogenes_company")
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO diogenes_company (cnpj_basico, name, trade_name, legal_nature, capital_cents,
                size, cnae, cnae_text, status, uf, city, opened_on)
         SELECT * FROM UNNEST($1::text[], $2::text[], $3::text[], $4::text[], $5::bigint[],
                $6::text[], $7::text[], $8::text[], $9::text[], $10::text[], $11::text[],
                $12::date[])",
    )
    .bind(&c.cnpj)
    .bind(&c.name)
    .bind(&c.trade_name)
    .bind(&c.nature)
    .bind(&c.capital)
    .bind(&c.size)
    .bind(&c.cnae)
    .bind(&c.cnae_text)
    .bind(&c.status)
    .bind(&c.uf)
    .bind(&c.city)
    .bind(&c.opened)
    .execute(&mut *tx)
    .await?;
    let known: HashSet<&String> = found.companies.keys().collect();
    let mut seen = HashSet::new();
    let (mut d_keys, mut d_cnpj, mut d_role, mut d_since) = (vec![], vec![], vec![], vec![]);
    let (mut p_ids, mut p_cnpj, mut p_role, mut p_since) = (vec![], vec![], vec![], vec![]);
    for link in &found.links {
        if !known.contains(&link.cnpj_basico) || !seen.insert((&link.target, &link.cnpj_basico)) {
            continue;
        }
        let role = found
            .roles
            .get(&link.role)
            .cloned()
            .unwrap_or_else(|| link.role.clone());
        match &link.target {
            Target::Donor(key) => {
                d_keys.push(key.as_str());
                d_cnpj.push(link.cnpj_basico.as_str());
                d_role.push(role);
                d_since.push(link.since);
            }
            Target::Deputy(id) => {
                p_ids.push(*id);
                p_cnpj.push(link.cnpj_basico.as_str());
                p_role.push(role);
                p_since.push(link.since);
            }
        }
    }
    // A donor that a new donations import removed has no row: the join skips the link.
    sqlx::query(
        "INSERT INTO diogenes_donor_company (election_year, donor_key, cnpj_basico, role, since)
         SELECT $1, l.k, l.c, l.r, l.s
           FROM UNNEST($2::text[], $3::text[], $4::text[], $5::date[]) AS l(k, c, r, s)
           JOIN diogenes_donor d ON d.election_year = $1 AND d.key = l.k",
    )
    .bind(ELECTION_YEAR)
    .bind(&d_keys)
    .bind(&d_cnpj)
    .bind(&d_role)
    .bind(&d_since)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO diogenes_deputy_company (camara_id, cnpj_basico, role, since)
         SELECT l.i, l.c, l.r, l.s
           FROM UNNEST($1::int[], $2::text[], $3::text[], $4::date[]) AS l(i, c, r, s)
           JOIN diogenes_deputy d ON d.camara_id = l.i",
    )
    .bind(&p_ids)
    .bind(&p_cnpj)
    .bind(&p_role)
    .bind(&p_since)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO diogenes_dataset (name, version, source_url, imported_at)
         VALUES ($1, $2, $3, now())
         ON CONFLICT (name) DO UPDATE
            SET version = EXCLUDED.version, source_url = EXCLUDED.source_url,
                imported_at = EXCLUDED.imported_at",
    )
    .bind(DATASET)
    .bind(month)
    .bind(url)
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}
