# ADR-0003: Get reference data from official sources and check it against official totals

- Status: Accepted
- Date: 2026-11-24

## Context

The first version of the vote count ("Placar de Votações") had a fixed list of deputies in
the page. People pasted JSON from the Câmara API by hand. The list became old, and the
transcription caused errors. The best defence was a check against the totals that the
source publishes.

## Decision

1. A sync job copies the data from the Câmara open-data API into the reference tables:
   - deputies in office, details and parties;
   - fronts and their members;
   - current leaders and the current board (Mesa Diretora);
   - plenary roll calls, votes, orientations and bills.
2. The key of a deputy is the official Câmara id. A name is never a key.
3. The colour/race value comes from the TSE 2022 candidate file. The job joins the two
   sources by CPF. The job does not store the CPF.
4. Each run records checks against official totals:
   - 513 deputies in office;
   - the number of women from the API filter and from the deputy details;
   - the votes of each roll call and the totals in its description;
   - the TSE matches and the deputies in office.
5. A failed check gives the status `warning`. The job keeps the data, and people can see
   the difference.
6. The client waits 350 ms between two requests. It retries HTTP 429 and 5xx, and it obeys
   `Retry-After`. The API sends HTTP 429 to fast clients.
7. The roll-call job uses windows of 85 days. The API accepts 3 months or less.
8. The official number of a front is its deputy signatures at creation. The job reads it
   from the status text. The status text is a record of events (creation, additions,
   removals). The members list of the API drops deputies who left office. These two numbers
   do not agree, so the job does not use them as a check. The job stores both numbers.

## Consequences

- The data is current without manual work.
- The first full sync takes several minutes, because of the wait between requests.
- The "Black and brown" caucus is self-declaration to the TSE. It is not the official
  Bancada Negra list. The interface tells the source.
- Data comes from the API with no manual step, so transcription errors do not occur.
- The board list has each member two times (specific role and "Titular"). The job keeps
  the specific role.
