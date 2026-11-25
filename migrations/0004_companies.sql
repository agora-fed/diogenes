-- DIOGENES 0004: companies of the donors and of the deputies (Receita Federal CNPJ open
-- data, ADR-0010).
--
-- The Receita file of partners ("sócios") shows a person with the name and the six middle
-- digits of the CPF ("***123456**"). DIOGENES keeps the same six digits for each person
-- donor, so that the `companies` job can find the donor in that file. The API never sends
-- them. A donations import gives new donor keys: it removes the donor links, and the
-- operator runs the `companies` job again.

ALTER TABLE diogenes_donor
    ADD COLUMN IF NOT EXISTS cpf_part text CHECK (cpf_part ~ '^[0-9]{6}$');

CREATE TABLE IF NOT EXISTS diogenes_company (
    cnpj_basico    text PRIMARY KEY CHECK (cnpj_basico ~ '^[0-9]{8}$'),
    name           text NOT NULL,
    trade_name     text,
    legal_nature   text,
    capital_cents  bigint CHECK (capital_cents >= 0),
    size           text NOT NULL DEFAULT 'unknown'
        CHECK (size IN ('micro', 'small', 'other', 'unknown')),
    cnae           text,
    cnae_text      text,
    status         text NOT NULL DEFAULT 'unknown'
        CHECK (status IN ('active', 'suspended', 'unfit', 'closed', 'null', 'unknown')),
    uf             text,
    city           text,
    opened_on      date
);

-- One row for each (donor, company) where the name and the six CPF digits are equal.
CREATE TABLE IF NOT EXISTS diogenes_donor_company (
    election_year  smallint NOT NULL,
    donor_key      text NOT NULL,
    cnpj_basico    text NOT NULL REFERENCES diogenes_company (cnpj_basico) ON DELETE CASCADE,
    role           text NOT NULL,
    since          date,
    PRIMARY KEY (election_year, donor_key, cnpj_basico),
    FOREIGN KEY (election_year, donor_key)
        REFERENCES diogenes_donor (election_year, key) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS diogenes_donor_company_cnpj_idx
    ON diogenes_donor_company (cnpj_basico);

-- The companies where a deputy in office is a partner (civil name and six CPF digits).
CREATE TABLE IF NOT EXISTS diogenes_deputy_company (
    camara_id      integer NOT NULL REFERENCES diogenes_deputy (camara_id) ON DELETE CASCADE,
    cnpj_basico    text NOT NULL REFERENCES diogenes_company (cnpj_basico) ON DELETE CASCADE,
    role           text NOT NULL,
    since          date,
    PRIMARY KEY (camara_id, cnpj_basico)
);

-- The version of an external data set (the Receita month, for example "2026-11").
CREATE TABLE IF NOT EXISTS diogenes_dataset (
    name         text PRIMARY KEY,
    version      text NOT NULL,
    source_url   text NOT NULL,
    imported_at  timestamptz NOT NULL DEFAULT now()
);

INSERT INTO diogenes_migration (version) VALUES (4) ON CONFLICT DO NOTHING;
