-- DIOGENES 0002: the approval rule of a project.
--
-- 'fixed'  = "yes" must reach the threshold (257 for a PLP, 308 for a PEC).
-- 'simple' = "yes" must be more than "no"; the threshold is the quorum (ordinary bills).
-- Existing projects keep 'fixed'. Their owners change the rule in the project settings.

ALTER TABLE diogenes_project
    ADD COLUMN IF NOT EXISTS rule text NOT NULL DEFAULT 'fixed'
        CHECK (rule IN ('fixed', 'simple'));

INSERT INTO diogenes_migration (version) VALUES (2) ON CONFLICT DO NOTHING;
