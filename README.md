# DIOGENES

Vote count and articulation rooms for the Brazilian Chamber of Deputies.
DIOGENES is an add-on for the [AGORA framework](https://github.com/agora-fed/core).
The first host is PINDORAMA (<https://democracia.social.br>).

License: AGPL-3.0-or-later.

## Credit

DIOGENES comes from the "Placar de Votações" vote count that Lucas Leffa designed.
DIOGENES uses the ideas and the rules of that design. It does not use its code.

## What DIOGENES does

DIOGENES has two parts.

**Public data.** A sync job copies public data from the Câmara dos Deputados and the TSE:

- the 513 deputies in office, with party, state and photo;
- parliamentary fronts and their members;
- caucuses: women, self-declared Black and brown (TSE 2022), current leaders, the board;
- plenary roll calls, with each vote, the party orientations and the bill.

Each sync run checks the stored data against the totals that the source publishes.
`GET /diogenes/sync/status` shows the result.

**Articulation rooms.** A room is a private work area for a team, in the area
`/articulacao/` of the host site. In a room, the team can:

- make one project for each bill or vote, with a threshold (257 by default);
- move deputies between columns: Sim, Não, Em disputa, Ausente, Presidente, Sem posição,
  and custom columns;
- keep many scenarios, and make a scenario from a real roll call;
- see the scoreboard and the outcome (approved, rejected, or the missing votes);
- see the dissidents of a roll call;
- filter by party, state, front and caucus, and group by party or state;
- mark priority deputies, and paste a list of names to mark them;
- write notes about a deputy, for the room or only for the author;
- see the full history of changes;
- see the changes of other members live.

## How the host mounts DIOGENES

```rust
use std::sync::Arc;

let state = diogenes::DiogenesState::new(pool.clone(), Arc::new(MyIdentity { /* ... */ }));
diogenes::migrate(&pool).await?;
let app = axum::Router::new()
    .nest("/api/v1", diogenes::api_router(state))
    .merge(diogenes::web_router());
let _sync = diogenes::sync::spawn_scheduler(pool, diogenes::sync::SyncConfig::from_env());
```

The host implements the `diogenes::Identity` trait. See ADR-0002.

## Configuration

| Variable | Default | Meaning |
|---|---|---|
| `DIOGENES_ROLL_CALLS_SINCE` | 180 days ago | The first day of the roll-call backfill (YYYY-MM-DD). |
| `DIOGENES_ROLL_CALLS_EVERY_MIN` | 60 | Minutes between two roll-call syncs. |
| `DIOGENES_REFERENCE_EVERY_HOURS` | 24 | Hours between two syncs of deputies and groups. |
| `DIOGENES_TSE_ENABLED` | true | Set `false` to skip the TSE colour/race import. |

## API

All routes start with `/diogenes/`. The response envelope is
`{ "success", "data", "error": { "code", "message" } }`.

| Method and path | Access |
|---|---|
| `GET deputies`, `GET groups`, `GET groups/{id}` | Public |
| `GET roll-calls`, `GET roll-calls/{id}`, `GET sync/status` | Public |
| `POST sync/{job}` | Sync operators |
| `GET/POST rooms` | Signed-in users (create: with permission) |
| `GET/PATCH/DELETE rooms/{id}`, `POST rooms/{id}/members`, `DELETE rooms/{id}/members/{user}` | Room members, by role |
| `GET rooms/{id}/audit`, `POST rooms/{id}/projects` | Room members, by role |
| `GET/PATCH/DELETE projects/{id}`, `POST projects/{id}/columns`, `PATCH/DELETE columns/{id}` | Room members, by role |
| `PUT projects/{id}/priorities`, `POST projects/{id}/notes`, `PATCH/DELETE notes/{id}` | Room members, by role |
| `POST projects/{id}/match-list`, `GET projects/{id}/events` (SSE) | Room members |
| `POST projects/{id}/scenarios`, `GET/PATCH/DELETE scenarios/{id}`, `PUT scenarios/{id}/positions` | Room members, by role |

Roles: `viewer` can read. `editor` can also change counts, priorities and notes.
`owner` can also manage members and delete the room.

## Development

1. Make a test database: `createdb diogenes_test`.
2. Run all tests:

   ```sh
   cargo fmt --all -- --check
   cargo clippy --all-targets -- -D warnings
   DATABASE_URL=postgres:///diogenes_test cargo test -- --include-ignored
   ```

3. Run the development server (one fixed user, local use only):

   ```sh
   createdb diogenes_dev
   DATABASE_URL=postgres:///diogenes_dev DIOGENES_DEV_SYNC=1 cargo run --example dev_server
   ```

4. Open <http://[::1]:8090/articulacao/>.

## Documentation

- Style: `docs/STYLE.md` (ASD-STE100). All documentation uses it.
- Decisions: `docs/adr/`.
