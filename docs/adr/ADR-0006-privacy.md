# ADR-0006: Protect notes and personal data

- Status: Accepted
- Date: 2026-11-24

## Context

The reference data is about public officials in their public role. The Câmara and the TSE
publish it. The notes of a team are the most sensitive data in DIOGENES. The Brazilian
data protection law (LGPD) applies to notes about people.

## Decision

1. A note has a visibility: `room` (all members) or `author` (only the author).
2. Only the author can change a note. The author or a room owner can delete a note.
3. The audit table never stores note text.
4. The API never sends the colour/race value of one deputy. Only the caucus membership
   shows it, as the TSE publishes it.
5. DIOGENES does not store the CPF.
6. A non-member gets HTTP 404 for a room, not HTTP 403. Nobody can test which rooms exist.
7. A room belongs to one org. A user of another org cannot see it.
8. The interface tells members to write only about the public role of a deputy.

## Consequences

- A legal review of the notes feature is necessary before a public launch.
- The room owner is responsible for the members and their notes.
