# ADR-0007: The host draws the pages and tells new members

- Status: Accepted
- Date: 2026-11-25
- Changes: ADR-0002 (the `web_router` part)

## Context

DIOGENES had its own pages at `/articulacao/`: HTML, CSS and JavaScript with no build step.
The pages did not use the design system of the host. In production, buttons went out of
their cards, fields cut their text, and the header did not match the site. People saw two
different products.

A room owner adds people to a room. The new member did not get a message. The member saw
the room only when they opened the articulation area.

## Decision

1. The host draws all pages of the articulation area with its own design system.
   DIOGENES gives the JSON API only.
2. The API does not change. The page addresses (`#/sala/…`, `#/projeto/…?cenario=…`,
   `#/votacoes`, `#/grupos`) do not change.
3. `web_router` and `src/web` stay for one release, so that a host can go back with one
   line. A later release removes them.
4. The `Identity` trait gets `member_added`. DIOGENES calls it after the change is saved,
   only for a new member. It does not call it for a role change or when an owner adds
   themself. The default does nothing.
5. The host decides how to tell the member (notification, e-mail, push) and applies the
   preferences of the member.

## Consequences

- The pages look and work like the rest of the host site, also in dark mode and on phones.
- A host without a design system must write its own pages for the API.
- DIOGENES does not send e-mail and does not store e-mail addresses (ADR-0006).
- The notice comes after the commit. If the host fails to send it, the member is still
  saved.
