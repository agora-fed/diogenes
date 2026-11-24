-- DIOGENES 0001: reference data (public) and articulation rooms (private).
--
-- Rules for this file and every later file:
--   * Every statement is idempotent. An operator can apply the file with psql,
--     and the embedded runner can apply it again with no error.
--   * The last statement records the version in diogenes_migration.
--   * All tables use the prefix "diogenes_". The host owns all other names.

CREATE TABLE IF NOT EXISTS diogenes_migration (
    version     integer PRIMARY KEY,
    applied_at  timestamptz NOT NULL DEFAULT now()
);

-- ---------------------------------------------------------------------------
-- Reference data. Source: Câmara dos Deputados open-data API and TSE.
-- The key of a deputy is the official Câmara id. Names are not keys.
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS diogenes_deputy (
    camara_id    integer PRIMARY KEY,
    name         text NOT NULL,
    civil_name   text,
    party        text NOT NULL,
    uf           char(2) NOT NULL,
    in_office    boolean NOT NULL DEFAULT false,
    status       text,
    sex          char(1),
    -- Self-declared colour/race from the TSE 2022 candidate file (joined by CPF).
    -- The CPF is used only during the sync. It is not stored.
    race         text,
    photo_url    text,
    email        text,
    updated_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS diogenes_deputy_in_office_idx ON diogenes_deputy (in_office);

CREATE TABLE IF NOT EXISTS diogenes_party (
    camara_id   integer PRIMARY KEY,
    acronym     text NOT NULL,
    name        text NOT NULL,
    updated_at  timestamptz NOT NULL DEFAULT now()
);

-- A group is a parliamentary front ("frente") or a caucus ("bancada").
-- id examples: 'front:55703', 'caucus:women', 'caucus:black-brown', 'caucus:leaders'.
CREATE TABLE IF NOT EXISTS diogenes_group (
    id              text PRIMARY KEY,
    kind            text NOT NULL CHECK (kind IN ('front', 'caucus')),
    camara_id       integer,
    title           text NOT NULL,
    source          text NOT NULL,
    -- The total that the source publishes. NULL when the source publishes no total.
    official_total  integer,
    details         jsonb NOT NULL DEFAULT '{}'::jsonb,
    updated_at      timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS diogenes_group_member (
    group_id   text NOT NULL REFERENCES diogenes_group (id) ON DELETE CASCADE,
    camara_id  integer NOT NULL,
    role       text,
    PRIMARY KEY (group_id, camara_id)
);
CREATE INDEX IF NOT EXISTS diogenes_group_member_deputy_idx ON diogenes_group_member (camara_id);

CREATE TABLE IF NOT EXISTS diogenes_bill (
    camara_id   integer PRIMARY KEY,
    kind        text NOT NULL,
    number      integer NOT NULL,
    year        integer NOT NULL,
    summary     text NOT NULL DEFAULT '',
    updated_at  timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS diogenes_roll_call (
    id             text PRIMARY KEY,
    voted_on       date NOT NULL,
    registered_at  timestamptz,
    body           text NOT NULL,
    description    text NOT NULL DEFAULT '',
    approved       boolean,
    bill_id        integer,
    -- Result of the check of the stored votes against the totals in the description.
    -- NULL = the description has no totals.
    totals_match   boolean,
    updated_at     timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS diogenes_roll_call_date_idx ON diogenes_roll_call (voted_on DESC);
CREATE INDEX IF NOT EXISTS diogenes_roll_call_bill_idx ON diogenes_roll_call (bill_id);

CREATE TABLE IF NOT EXISTS diogenes_roll_call_vote (
    roll_call_id  text NOT NULL REFERENCES diogenes_roll_call (id) ON DELETE CASCADE,
    camara_id     integer NOT NULL,
    vote          text NOT NULL,
    party         text NOT NULL,
    uf            char(2) NOT NULL,
    PRIMARY KEY (roll_call_id, camara_id)
);

CREATE TABLE IF NOT EXISTS diogenes_roll_call_orientation (
    roll_call_id  text NOT NULL REFERENCES diogenes_roll_call (id) ON DELETE CASCADE,
    bloc          text NOT NULL,
    bloc_kind     text,
    orientation   text NOT NULL,
    PRIMARY KEY (roll_call_id, bloc)
);

CREATE TABLE IF NOT EXISTS diogenes_sync_run (
    id           bigserial PRIMARY KEY,
    job          text NOT NULL,
    started_at   timestamptz NOT NULL DEFAULT now(),
    finished_at  timestamptz,
    status       text NOT NULL DEFAULT 'running'
                 CHECK (status IN ('running', 'ok', 'warning', 'failed')),
    items        integer NOT NULL DEFAULT 0,
    -- Each check: {"subject": ..., "expected": n, "actual": n}.
    checks       jsonb NOT NULL DEFAULT '[]'::jsonb,
    error        text
);
CREATE INDEX IF NOT EXISTS diogenes_sync_run_job_idx ON diogenes_sync_run (job, started_at DESC);

-- ---------------------------------------------------------------------------
-- Articulation rooms. Private to the members of each room.
-- user ids are host user ids (UUID). The host owns the user table.
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS diogenes_room (
    id          uuid PRIMARY KEY,
    org_id      uuid NOT NULL,
    name        text NOT NULL CHECK (length(name) BETWEEN 1 AND 120),
    created_by  uuid NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS diogenes_room_org_idx ON diogenes_room (org_id);

CREATE TABLE IF NOT EXISTS diogenes_room_member (
    room_id       uuid NOT NULL REFERENCES diogenes_room (id) ON DELETE CASCADE,
    user_id       uuid NOT NULL,
    display_name  text NOT NULL,
    role          text NOT NULL CHECK (role IN ('owner', 'editor', 'viewer')),
    added_at      timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (room_id, user_id)
);
CREATE INDEX IF NOT EXISTS diogenes_room_member_user_idx ON diogenes_room_member (user_id);

CREATE TABLE IF NOT EXISTS diogenes_project (
    id          uuid PRIMARY KEY,
    room_id     uuid NOT NULL REFERENCES diogenes_room (id) ON DELETE CASCADE,
    name        text NOT NULL CHECK (length(name) BETWEEN 1 AND 120),
    threshold   integer NOT NULL DEFAULT 257 CHECK (threshold BETWEEN 1 AND 513),
    bill_id     integer,
    created_by  uuid NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS diogenes_project_room_idx ON diogenes_project (room_id);

-- Fixed columns have a key: undecided, yes, no, disputed, absent, chair.
-- Custom columns have key NULL.
CREATE TABLE IF NOT EXISTS diogenes_column (
    id          uuid PRIMARY KEY,
    project_id  uuid NOT NULL REFERENCES diogenes_project (id) ON DELETE CASCADE,
    key         text,
    name        text NOT NULL CHECK (length(name) BETWEEN 1 AND 60),
    color       text NOT NULL DEFAULT '#888888',
    position    integer NOT NULL DEFAULT 0,
    UNIQUE (project_id, key)
);

CREATE TABLE IF NOT EXISTS diogenes_scenario (
    id                 uuid PRIMARY KEY,
    project_id         uuid NOT NULL REFERENCES diogenes_project (id) ON DELETE CASCADE,
    name               text NOT NULL CHECK (length(name) BETWEEN 1 AND 120),
    source_roll_call   text,
    created_by         uuid NOT NULL,
    created_at         timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS diogenes_scenario_project_idx ON diogenes_scenario (project_id);

-- One row per deputy per scenario. No row = the deputy is in the "undecided" column.
-- When a custom column is deleted, its rows go (ON DELETE CASCADE): the deputies become undecided.
CREATE TABLE IF NOT EXISTS diogenes_position (
    scenario_id  uuid NOT NULL REFERENCES diogenes_scenario (id) ON DELETE CASCADE,
    camara_id    integer NOT NULL,
    column_id    uuid NOT NULL REFERENCES diogenes_column (id) ON DELETE CASCADE,
    updated_by   uuid NOT NULL,
    updated_at   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (scenario_id, camara_id)
);

CREATE TABLE IF NOT EXISTS diogenes_priority (
    project_id  uuid NOT NULL REFERENCES diogenes_project (id) ON DELETE CASCADE,
    camara_id   integer NOT NULL,
    marked_by   uuid NOT NULL,
    marked_at   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (project_id, camara_id)
);

-- visibility 'room' = all members can read; 'author' = only the author can read.
CREATE TABLE IF NOT EXISTS diogenes_note (
    id          uuid PRIMARY KEY,
    project_id  uuid NOT NULL REFERENCES diogenes_project (id) ON DELETE CASCADE,
    camara_id   integer NOT NULL,
    author_id   uuid NOT NULL,
    body        text NOT NULL CHECK (length(body) BETWEEN 1 AND 10000),
    visibility  text NOT NULL DEFAULT 'room' CHECK (visibility IN ('room', 'author')),
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS diogenes_note_project_idx ON diogenes_note (project_id, camara_id);

-- Append-only audit trail of every change in a room.
CREATE TABLE IF NOT EXISTS diogenes_audit (
    id        bigserial PRIMARY KEY,
    room_id   uuid NOT NULL REFERENCES diogenes_room (id) ON DELETE CASCADE,
    actor_id  uuid NOT NULL,
    action    text NOT NULL,
    payload   jsonb NOT NULL DEFAULT '{}'::jsonb,
    at        timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS diogenes_audit_room_idx ON diogenes_audit (room_id, id DESC);

INSERT INTO diogenes_migration (version) VALUES (1) ON CONFLICT DO NOTHING;
