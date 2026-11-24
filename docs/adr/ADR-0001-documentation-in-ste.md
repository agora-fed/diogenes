# ADR-0001: Write documentation in ASD-STE100

- Status: Accepted
- Date: 2026-11-24

## Context

The DIOGENES documentation has many readers. Some readers do not speak English as a first
language. Some readers are software agents that read and write code and text.

Long and unclear text causes errors. Different words for one thing cause errors too.

## Decision

1. Write all documentation and all ADRs in ASD-STE100 Simplified Technical English.
2. Use the rules and the glossary in `docs/STYLE.md`.
3. Keep the user interface text in Brazilian Portuguese. STE does not apply to it.

## Consequences

- A reviewer checks each pull request with the check list in `docs/STYLE.md`.
- New terms go into the glossary before they go into other files.
- The text is shorter. Some explanations need more sentences.
