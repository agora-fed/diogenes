-- DIOGENES 0005: the number of other people with the same name in the Receita file of
-- partners, for each company link (ADR-0010). The page shows it next to the link: with
-- a common name, a false link is more probable.

ALTER TABLE diogenes_donor_company
    ADD COLUMN IF NOT EXISTS namesakes integer NOT NULL DEFAULT 0 CHECK (namesakes >= 0);
ALTER TABLE diogenes_deputy_company
    ADD COLUMN IF NOT EXISTS namesakes integer NOT NULL DEFAULT 0 CHECK (namesakes >= 0);

INSERT INTO diogenes_migration (version) VALUES (5) ON CONFLICT DO NOTHING;
