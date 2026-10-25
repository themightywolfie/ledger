# Domain docs

This repo has a single context: one `GLOSSARY.md` at the root and decisions in `docs/adr/`.

## Before exploring, read these

- `GLOSSARY.md` at the repo root.
- `docs/adr/`: the ADRs that touch the area you are working in.
- `docs/design/`: the architecture, the integrity rules and the public contracts. Read the section a ticket references.

## Use the glossary's vocabulary

When your output names a domain concept (a ticket title, a test name, a type, a log message), use the term as `GLOSSARY.md` defines it, and avoid the synonyms it lists under _Avoid_. For example, say "claim", not "memory" or "fact"; "journal entry", not "event" or "row"; "forget", not "delete".

If the concept you need isn't in the glossary yet, that's a signal. Either you are inventing language the project doesn't use (reconsider), or there's a real gap (raise it so the glossary can be updated).

## Flag ADR conflicts

If your change contradicts an ADR, say so explicitly rather than overriding it silently, for example: "Contradicts ADR-0003 (append-only journal), but worth reopening because…"
