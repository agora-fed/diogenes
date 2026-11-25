-- DIOGENES 0003: campaign finance of the deputies in office (TSE, ADR-0009).
--
-- The values are in cents. A donor has no CPF or CNPJ here: the key is a keyed hash
-- with a new random salt for each import. The import replaces all rows of the year.

CREATE TABLE IF NOT EXISTS diogenes_campaign_total (
    election_year  smallint NOT NULL,
    camara_id      integer NOT NULL REFERENCES diogenes_deputy (camara_id) ON DELETE CASCADE,
    source         text NOT NULL
        CHECK (source IN ('individuals', 'party', 'own', 'candidates', 'crowdfunding', 'other')),
    cents          bigint NOT NULL CHECK (cents >= 0),
    receipts       integer NOT NULL CHECK (receipts >= 0),
    PRIMARY KEY (election_year, camara_id, source)
);

CREATE TABLE IF NOT EXISTS diogenes_donor (
    election_year  smallint NOT NULL,
    key            text NOT NULL,
    kind           text NOT NULL CHECK (kind IN ('person', 'candidate')),
    name           text NOT NULL,
    activity       text,
    uf             text,
    PRIMARY KEY (election_year, key)
);

-- via_party = true: the person gave the money to a party, and the party sent it to the deputy.
CREATE TABLE IF NOT EXISTS diogenes_donation (
    election_year  smallint NOT NULL,
    camara_id      integer NOT NULL REFERENCES diogenes_deputy (camara_id) ON DELETE CASCADE,
    donor_key      text NOT NULL,
    via_party      boolean NOT NULL,
    cents          bigint NOT NULL CHECK (cents >= 0),
    receipts       integer NOT NULL CHECK (receipts >= 0),
    PRIMARY KEY (election_year, camara_id, donor_key, via_party),
    FOREIGN KEY (election_year, donor_key)
        REFERENCES diogenes_donor (election_year, key) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS diogenes_donation_donor_idx
    ON diogenes_donation (election_year, donor_key);

INSERT INTO diogenes_migration (version) VALUES (3) ON CONFLICT DO NOTHING;
