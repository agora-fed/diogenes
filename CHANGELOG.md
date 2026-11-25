# Changelog

All changes to DIOGENES are in this file. The format follows Keep a Changelog.
The versions follow Semantic Versioning.

## [Unreleased]

### Added

- Companies of the donors and of the deputies from the Receita Federal CNPJ open data: the
  sync job `companies` (operator only, after `donations`), the companies in the campaign of
  a deputy, and the shared donors and the sectors of a scenario (ADR-0010, migration 0004).
- `GET /diogenes/campaigns`: the deputies in office with the total of their campaign.
- Campaign finance from the TSE (2022) for the deputies in office: the sync job `donations`
  (operator only), totals by source, donors (with the original donors of party money), and
  the donors that a scenario shares across its columns. No CPF or CNPJ is stored
  (ADR-0009, migration 0003).
- The approval rule of a project: `fixed` (a number of votes) or `simple` (more "yes" than
  "no", with a quorum). The kind of the bill sets the default: PEC 308, PLP 257, other
  kinds simple majority (ADR-0008, migration 0002).
- `Identity::member_added` and the `MemberAdded` notice. DIOGENES calls it when a room
  owner adds a new member. The default does nothing (ADR-0007).

### Deprecated

- `web_router` and the pages in `src/web`. The host draws the pages now (ADR-0007).

### Fixed

- A sync job now frees its advisory lock when the database refuses the run row.
  Before, the lock stayed on a pooled connection until the process ended.

### Changed

- The campaign of a deputy is public. Before, a signed-in user of an org with the module on
  could read it (ADR-0009, change of 2026-11-25).
- The donations import keeps the six middle CPF digits of each person donor, the part that
  the Receita publishes. The API never sends them (ADR-0010).

- The CI gate for line coverage is now 95 percent (was 90).
- New tests cover the Câmara client retries, the TSE import, the sync jobs and locks,
  the scheduler, live-stream limits and the schema runner.
- New function `sync::deputies::run_with_races`. It takes the colour/race map as an
  argument. `sync::deputies::run` is unchanged.

## [0.1.0] - 2026-11-24

### Added

- Sync of public data from the Câmara API: deputies in office, parties, fronts,
  leaders, board, plenary roll calls, votes, orientations and bills.
- Import of self-declared colour/race from the TSE 2022 file. The join uses the CPF.
  DIOGENES does not store the CPF.
- Checks against official totals in each sync run.
- Articulation rooms: members and roles, projects, columns, scenarios, positions,
  priorities, notes, audit trail and live updates (SSE).
- Scenario from a real roll call, with dissidents.
- The area `/articulacao/` (HTML, CSS and JavaScript, no build step).
- Public read API for deputies, groups and roll calls.
