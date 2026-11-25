# ADR-0010: Companies of the donors and of the deputies, from the Receita Federal

- Status: Accepted
- Date: 2026-11-25
- Changes: ADR-0006 (personal data), ADR-0009 (campaign finance)

## Context

Since 2015 a company cannot give money to a campaign. The money of a sector comes through
people: the partners and the managers of the companies. To see a sector behind the
donations, DIOGENES must find the companies of each donor.

The Receita Federal publishes the CNPJ register as open data, one set each month. Facts:

- The files are on a public share (`arquivos.receitafederal.gov.br`, WebDAV). Each month
  has 10 parts of partners ("Sócios", approximately 650 MB), 10 parts of companies
  ("Empresas", approximately 1.4 GB) and 10 parts of establishments
  ("Estabelecimentos", approximately 5.5 GB), and small code tables.
- Each part is a ZIP file with one CSV entry, latin-1, `;` separators, no header.
  The largest part is larger than 2 GB, with Zip64 sizes.
- The partner file shows a person CPF as `***123456**`: only the six middle digits.
- A partner is not always an owner. The file has a role ("qualificação"): partner,
  managing partner, director, administrator, and others. In a company with shares
  ("S.A."), the file lists the directors, not the shareholders.
- The file has the partners of today. A person that left a company is not in it.
- A company can be a partner of another company (a holding).

## Decision

1. A new sync job, `companies`. Only an operator starts it, after `donations`. It does not
   run on a schedule.
2. Match: the normalized name (no diacritics, lower case, single spaces) and the six middle
   CPF digits must be equal. The donations import keeps these six digits for each person
   donor (`diogenes_donor.cpf_part`, migration 0004). The Receita publishes the same six
   digits with the name, so the column adds no new personal data. The API never sends it.
3. Two donors with the same name and the same six digits are ambiguous. The job does not
   link them, and counts them in the check `ambiguous_donors`.
4. The job also finds the companies of each deputy in office: civil name and CPF from the
   Câmara API, in memory only.
5. The job reads the parts as streams from the HTTP body. It reads the local ZIP header,
   then inflates the data that follows. No part goes to the disk. Each part goes through
   a bounded channel to a blocking reader.
6. Order: partners (find the companies), companies (name, legal nature, capital, size),
   establishments (head office only: main activity, status, place, start date), code
   tables (roles, legal natures, activities, cities).
7. The job replaces all company rows in one transaction. A failed run changes nothing.
   Storage: `diogenes_company`, `diogenes_donor_company`, `diogenes_deputy_company`, and
   the month in `diogenes_dataset`.
8. A donations import gives new donor keys (ADR-0009). The foreign key removes the donor
   links. The operator runs `companies` again after each `donations` run.
9. Configuration: `DIOGENES_RFB_CNPJ_URL` (WebDAV root), `DIOGENES_RFB_CNPJ_SHARE` (the
   public share token, sent as the user name) and `DIOGENES_RFB_CNPJ_MONTH` (`YYYY-MM`).
   Without a month, the job reads the last month of the share.
10. API: the campaign of a deputy gives the companies of each donor (at most 5) and the
    companies of the deputy. The money of a scenario gives the companies of each shared
    donor (at most 3) and the main activities ("sectors") of the companies of the donors,
    with the deputies of each column. Each answer has the Receita month.
11. Each company link shows the role as the Receita writes it, and the notice that the name
    and six CPF digits are equal. DIOGENES never writes "owner", and never computes a
    judgement (ADR-0009, decision 8).

## Consequences

- One run reads approximately 7.5 GB from the Receita. It takes some tens of minutes.
- Checks: `files_read` (34), `companies_with_name`, `companies_with_head_office`,
  `ambiguous_donors`, `rejected_rows`. A difference gives the status `warning`.
- Limits that the page must state:
  - the partners are those of the Receita month, not of 2022;
  - a former partner is not found;
  - the job does not follow a holding to the companies that it owns;
  - two different people with the same name and the same six digits give a false link.
    This is rare, but possible.
- The share token comes from the Receita page. If the Receita moves the files, the operator
  sets the new location with the environment variables.
