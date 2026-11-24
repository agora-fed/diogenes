# ADR-0004: Store one row per position, audit all changes, send live updates with SSE

- Status: Accepted
- Date: 2026-11-24

## Context

The first version stored all positions of a scenario in one document. Two people who
changed the same list at the same time lost changes ("last write wins" on the full list).
The first version had no history and no undo.

## Decision

1. Store one row per deputy per scenario (`diogenes_position`). No row means "undecided".
2. A move of many deputies is one transaction.
3. The last write wins for one cell. The row keeps `updated_by` and `updated_at`.
4. Each change writes one row in `diogenes_audit`. The audit row of a note has no note text.
5. When a custom column goes, its positions go too (`ON DELETE CASCADE`). Those deputies
   become undecided.
6. Live updates use server-sent events (SSE), one channel per project. An event tells what
   changed. The page then loads the data again.
7. The channels live in the memory of one process.

## Consequences

- Two people who move different deputies never lose changes.
- The audit table gives a full history. An undo function can use it later.
- With more than one host process, a page sees only the changes of its own process. The
  page sees all changes when it loads again. PostgreSQL `LISTEN/NOTIFY` can remove this
  limit later.
- CRDT is not necessary at this scale (513 deputies, small teams).
