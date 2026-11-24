# Contributing

## Before you start

1. Read `docs/STYLE.md`. All documentation, comments and commit messages use ASD-STE100.
2. Read the ADRs in `docs/adr/`.

## Rules for code

- Write identifiers, routes and JSON fields in English.
- Write user interface text in Brazilian Portuguese.
- Use bound parameters in all SQL. Do not put user input into SQL text.
- Build the DOM with `h()` and `textContent` in `src/web/app.js`. Do not use `innerHTML` with data.
- Add a test for each change. Add an integration test for each new query.
- Keep the key of a deputy as the Câmara id. Do not use a name as a key.

## Before a pull request

1. Run `cargo fmt --all -- --check`.
2. Run `cargo clippy --all-targets -- -D warnings`.
3. Run `DATABASE_URL=postgres:///diogenes_test cargo test -- --include-ignored`.
4. Write the commit message as `<type>: <description>`. Types: feat, fix, refactor, docs,
   test, chore, perf, ci.
