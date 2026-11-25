# Changelog

All changes to DIOGENES are in this file. The format follows Keep a Changelog.
The versions follow Semantic Versioning.

## [Unreleased]

### Added

- `Identity::member_added` and the `MemberAdded` notice. DIOGENES calls it when a room
  owner adds a new member. The default does nothing (ADR-0007).

### Deprecated

- `web_router` and the pages in `src/web`. The host draws the pages now (ADR-0007).

### Fixed

- A sync job now frees its advisory lock when the database refuses the run row.
  Before, the lock stayed on a pooled connection until the process ended.

### Changed

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
