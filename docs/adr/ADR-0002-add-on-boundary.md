# ADR-0002: Make DIOGENES an add-on with no AGORA crate dependencies

- Status: Accepted
- Date: 2026-11-24

## Context

DIOGENES runs inside an AGORA host. The first host is PINDORAMA (democracia.social.br).

The `l10n-brazil` module depends on AGORA crates with a git dependency. The host also
keeps a copy of the module in its workspace. Two copies need a drift check.

If DIOGENES uses AGORA types, and the host uses its local path crates, Cargo builds two
copies of the same crate. The host types and the DIOGENES types are then different types.

## Decision

1. DIOGENES has no dependency on any AGORA crate.
2. The host gives DIOGENES a PostgreSQL pool and an implementation of the `Identity` trait.
3. The `Identity` trait has three functions:
   - `actor`: return the signed-in caller, with the user id, the org id and two permissions.
   - `find_user`: find a user of the same org by a public handle.
   - `module_active`: tell if the module is on for an org.
4. The host merges `api_router` under its API prefix and `web_router` at the site root.
5. DIOGENES never reads host cookies, host tokens or host tables.

## Consequences

- The host adds DIOGENES as one git dependency. No copy and no drift check are necessary.
- The host writes a small adapter (approximately 100 lines) for `Identity`.
- DIOGENES has its own response envelope with the same JSON shape as the AGORA envelope.
- Another host (not AGORA) can also mount DIOGENES.
