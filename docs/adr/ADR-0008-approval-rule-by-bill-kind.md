# ADR-0008: The approval rule follows the kind of the bill

- Status: Accepted
- Date: 2026-11-25
- Changes: ADR-0004 (the threshold of a project)

## Context

A project had one number: the threshold. "Yes" had to reach that number. The default was
257. This is correct for a complementary law (PLP, absolute majority, CF art. 69). It is
not correct for an ordinary bill (PL), a provisional measure (MPV) or a legislative decree
(PDL). For these kinds, the Chamber approves with more "yes" than "no", when a quorum of
257 deputies is present (CF art. 47, RICD art. 183). For a constitutional amendment (PEC)
the number is 308 (CF art. 60).

In production, a board for an ordinary bill showed "open" after the Chamber approved the
bill with a large margin.

## Decision

1. A project has a rule: `fixed` or `simple`.
   - `fixed`: "yes" must reach the threshold.
   - `simple`: "yes" must be more than "no". The threshold is the quorum.
2. When the caller sends no rule and no threshold, the kind of the stored bill sets them:
   PEC → `fixed`, 308; PLP → `fixed`, 257; all other kinds → `simple`, quorum 257.
   With no bill, or a bill that is not stored: `fixed`, 257 (as before).
3. A rule or a threshold that the caller sends always wins. Some votes on an ordinary bill
   need an absolute majority (for example an urgency request, RICD art. 155). The team sets
   `fixed` for these projects.
4. The count for `simple`:
   - "Open" deputies are "disputed", "undecided" and custom columns. They can vote either way
     or stay away. "Absent" deputies do not vote. The chair counts for the quorum only.
   - Approved: "yes" is more than "no" plus all open deputies, and "yes" + "no" + chair
     reaches the quorum.
   - Rejected: "yes" plus all open deputies is not more than "no", or the quorum is not
     possible. A tie is not approval.
   - Open: all other cases. `missing` is the number of open deputies that must vote "yes"
     so that "yes" wins. `quorum_missing` is the number of present deputies that the quorum
     still needs.
5. Migration 0002 adds the column with the default `fixed`. Existing projects do not change.
   Their owners change the rule in the project settings.

## Consequences

- The API adds `rule` to the project, the room project list and the scenario. The outcome
  `open` adds `quorum_missing` (always 0 for `fixed`).
- The host must show the rule and let editors change it.
- The count is conservative: a custom column (for example "Abstenção") counts as open.
