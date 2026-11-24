# Changelog

All changes to DIOGENES are in this file. The format follows Keep a Changelog.
The versions follow Semantic Versioning.

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
