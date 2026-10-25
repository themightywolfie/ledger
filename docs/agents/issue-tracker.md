# Issue tracker: Trello

Tickets for this repo live on the **Ledger** Trello board: https://trello.com/b/mXDl1YY8/ledger. Use the Trello tools (read and write cards, lists and checklists) to work with it.

Specs stay in the repo, at `.scratch/<feature>/spec.md`. The board holds the tickets that implement them. Each card points back to its spec, user stories and design sections.

## Board layout

Lists, left to right:

- **About this board**: workflow notes and the colour legend.
- **Later**: milestones after v1.
- **Blocked**: tickets with at least one blocker not yet Done.
- **Ready**: the frontier. Every blocker is Done, so the card can start now.
- **In progress**, **In review**, **Done**.

Trello's default labels are used as area tags, by colour:

| Colour | Area |
| --- | --- |
| Red | Foundations and CI |
| Green | Engine and integrity rules |
| Yellow | Verifiers |
| Orange | Storage, recall, backup |
| Blue | MCP, REST and CLI surfaces |
| Purple | Memory visualizer |

## Ticket format

- **Name**: `#NN Title`, numbered in dependency order (blockers first). v1 is #01–#19 for core and #20–#28 for the visualizer.
- **Description**: `**What to build**` (end-to-end behaviour, not a layer list), `**Blocked by**` (the `#NN Title` of each blocking card, or "None (can start immediately)"), `**References**` (spec stories, design sections, ADRs), and a `Status:` line.
- **Checklist** named "Acceptance criteria": one item per criterion.
- **Label**: one area colour.

## When a skill says…

- **"Publish to the issue tracker"**: create a card in Blocked (or in Ready if nothing blocks it) in the format above, with an "Acceptance criteria" checklist and an area label. Number it after the highest existing `#NN`.
- **"Fetch the relevant ticket"**: search cards on the Ledger board by `#NN` or title, then read the card with its checklist and comments.
- **"Claim a ticket"**: move the card to In progress before starting work.
- **"Close a ticket"**: check every acceptance item, add a comment linking the commit or PR, move the card to Done and mark it complete. Then move every Blocked card whose blockers are now all Done to Ready.
- **Comments and conversation**: add them as card comments.
