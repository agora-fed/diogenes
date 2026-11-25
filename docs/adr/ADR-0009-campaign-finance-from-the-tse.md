# ADR-0009: Campaign finance from the TSE

- Status: Accepted
- Date: 2026-11-25
- Changes: ADR-0003 (data sources), ADR-0006 (personal data)

## Context

A team in a room wants to know who paid for the campaigns of the deputies in a scenario.
The TSE publishes the accounts of all candidates ("prestação de contas eleitorais") as open
data. The deputies in office were elected in 2022.

Facts about the 2022 data:

- Since 2015 (STF, ADI 4650), a company cannot give money to a campaign. The money comes
  from parties (mostly public funds), people, the candidate, other candidates and
  crowdfunding.
- A person can give money to a party, and the party can send it to a candidate. The TSE
  publishes these original donors in a separate file.
- The archive has approximately 500 MB. The national receipt file has approximately 430 MB.
- Each candidate has one set of accounts. A receipt number (`SQ_RECEITA`) can occur on more
  than one row of the same set. Each row is a real receipt. DIOGENES adds all rows.

## Decision

1. A new sync job, `donations`, imports the 2022 accounts of the deputies in office.
   Only an operator starts it (`POST /diogenes/sync/donations`). It does not run on a
   schedule. `DIOGENES_TSE_FINANCE_URL` can set another archive.
2. The job gets the CPF of each deputy from the Câmara API and keeps it in memory only.
3. The job downloads the archive to a temporary file and reads the CSV as a stream. It
   refuses an archive larger than 2 GB. It never cuts a file. A row with a bad value is
   counted in the check `rejected_rows`. The job deletes the file at the end.
4. The job replaces all rows of the year in one transaction. A failed run changes nothing.
5. Storage (migration 0003, values in cents):
   - `diogenes_campaign_total`: the money of each deputy by source (individuals, party,
     own, candidates, crowdfunding, other);
   - `diogenes_donor`: people and campaigns of other candidates that gave money;
   - `diogenes_donation`: the money from one donor to one deputy, direct or through a
     party (`via_party`).
6. DIOGENES does not store the CPF or the CNPJ of a donor. The donor key is a SHA-256 hash
   of the document with a random salt. The salt exists only during one import, so a key
   changes at each import and nobody can test a CPF against it.
7. API:
   - `GET /diogenes/deputies/{id}/campaign`: totals by source and the 15 largest donors.
     A signed-in user of an org with the module on can read it.
   - `GET /diogenes/scenarios/{id}/donors`: the money of each column, and the donors that
     gave to two or more deputies in office, with the number of deputies in each column.
     Only members of the room can read it.
   - Each answer has the election year, the link to the TSE data set and the import date.
8. DIOGENES shows declared facts with their source. It does not compute a judgement about a
   person ("influenced", "lobbyist", a score). A donation does not prove influence.
   The team reads the facts and makes its own analysis.

## Consequences

- An operator must start the job once after the deploy, and again when the TSE publishes a
  correction. One run takes approximately 3 minutes (513 Câmara requests and the archive).
- A link between a person and a company needs a second data set (the partners of companies,
  Receita Federal). This ADR does not include it.
- The names of individual donors are personal data that the TSE publishes for public
  control of elections (LGPD, art. 7, § 3 and § 4). DIOGENES uses them for the same purpose.
  The host decides who can see them; the default is signed-in users only.
- The 2026 accounts are for the next legislature. A later change can add the year as a
  parameter.
