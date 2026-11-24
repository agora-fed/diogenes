# Documentation style: ASD-STE100

All DIOGENES documentation and all ADRs use ASD-STE100 Simplified Technical English (STE).
This file gives the rules. ADR-0001 records the decision.

## Scope

- Apply STE to: `README.md`, `docs/**`, ADRs, `CHANGELOG.md`, `CONTRIBUTING.md`,
  `SECURITY.md`, code comments and doc comments, commit messages and pull requests.
- Do not apply STE to: text that people see in the user interface. That text is in
  Brazilian Portuguese (pt-BR), because the users are in Brazil.
- Write documentation in English. STE is an English standard.

## Source of these rules

The current edition is ASD-STE100 Issue 9 (January 2025). It has 53 writing rules and a
dictionary of approximately 900 approved words. The rules below are a summary from public
secondary sources. They are not a copy of the standard. Get the standard from
<https://www.asd-ste100.org/> for the full rules and the dictionary.

## Words

1. Use approved words only, with the meaning and part of speech in the STE dictionary.
2. Use one word for one meaning. Do not use synonyms for variety.
   - Example: use "deputy" every time. Do not change to "member", "MP" or "lawmaker".
3. Technical names and technical verbs are permitted. Examples: "roll call", "scenario",
   "migration", "sync", "endpoint", "advisory lock".
4. Do not write a noun cluster of more than three words.
   - Not STE: "deputy position audit trail table".
   - STE: "the audit table for deputy positions".
5. Use the "-ing" form only in a technical name ("polling interval"), not as a verb form.

## Verbs and grammar

6. Use only these verb forms: infinitive, imperative, simple present, simple past,
   simple future, and the past participle as an adjective.
7. Use the active voice. In descriptive text, use the passive only when the agent is unknown.
   - Not STE: "The positions are written by the handler."
   - STE: "The handler writes the positions."
8. Do not make complex verb structures with auxiliary verbs ("should have been done").
9. Do not omit words to make text shorter. Keep articles ("the", "a"), subjects and verbs.

## Sentences

10. Procedural sentences (instructions): 20 words or fewer.
11. Descriptive sentences: 25 words or fewer.
12. Write one instruction in one sentence.
13. Start an instruction with the verb: "Run the tests." Not: "You should run the tests."
14. Put a condition before the instruction: "If the check fails, open an issue."
15. Start a warning with a clear command or condition: "Do not store the CPF."

## Paragraphs and layout

16. Write one topic in one paragraph.
17. Write six sentences or fewer in one paragraph.
18. Use vertical lists for steps, options and complex text.
19. Use numbered lists for steps that have an order. Use bullets for items with no order.

## Check list for a reviewer

- [ ] Each sentence has 20 words or fewer (procedure) or 25 words or fewer (description).
- [ ] Each sentence is active voice, or the agent is unknown.
- [ ] One term has one meaning in all files (see the glossary).
- [ ] No noun cluster has more than three words.
- [ ] No paragraph has more than six sentences.
- [ ] Instructions start with a verb, one instruction per sentence.

## Glossary

Use these terms with these meanings only.

| Term | Meaning |
|---|---|
| deputy | A member of the Brazilian Chamber of Deputies (Câmara dos Deputados). |
| in office | A deputy who holds the seat now ("em exercício"). |
| room | A private work area for a team. It has members and projects. |
| member | A host user who has a role in a room. |
| project | One count for one bill or one vote. It has columns, priorities, notes and scenarios. |
| scenario | One set of positions in a project. |
| position | The column of one deputy in one scenario. |
| column | A category in a project, for example "Sim" or "Em disputa". |
| priority | A deputy that the team marks as important for a project. |
| note | Text that a member writes about a deputy in a project. |
| roll call | A recorded plenary vote ("votação nominal"). |
| orientation | The vote that a party or bloc leader recommends. |
| dissident | A deputy who votes "Sim" or "Não" against a "Sim" or "Não" orientation. |
| group | A front ("frente parlamentar") or a caucus ("bancada"). |
| official total | A number that the source publishes, for a check of the stored data. |
| host | The AGORA installation that mounts DIOGENES. |
| sync | The job that copies public data into the reference tables. |
