# Spec: Memory visualizer v1

**Status:** ready-for-agent
**Depends on:** `.scratch/ledger-core/spec.md` (the REST API it consumes)

## Problem Statement

Ledger's memory is structured, versioned and audited, but an operator looking after it has only a CLI and raw JSON. They can't easily see:

- what their agents currently believe, and how far each belief is trusted
- why a claim is trusted or not
- how a fact changed over time
- which conflicts and confirmations are waiting on a human

Fixing a wrong memory, forgetting something on request, or taking and restoring a backup means memorising commands and claim ids. Without a view of memory, the integrity Ledger provides is invisible, and the people who need to trust it (engineering leads, platform and security engineers, the users the agents act for) have no way to look.

## Solution

A small web app, the **memory visualizer**, built with React and TypeScript and compiled into the `ledgermem` binary. `ledgermem serve` serves it on localhost at `/ui` and prints a sign-in link. An operator can:

- **see** memory: browse and search claims; open a claim to read its trust breakdown, evidence, version-chain timeline and journal entries; and follow conflicts, open questions and each scope's journal with its chain-verification badge
- **modify** memory the Ledger way, always by appending: correct a claim (a new user-stated claim that supersedes it), retract, re-verify, forget, release from quarantine, settle conflicts and answer open questions
- **back up and restore**: download a tenant's backup bundle, and restore one into an empty tenant with a full report

The visualizer uses only the public REST API, so everything it does an integrator can do too.

## User Stories

### Signing in and orientation

1. As an operator, I want to open the visualizer from the link `ledgermem serve` prints, so that I'm signed in without copying keys around.
2. As an operator of a remote server, I want to paste an operator key to sign in, so that I can review memory that isn't on my machine.
3. As an operator, I want the key removed from the address bar and kept only for this browser session, so that it doesn't linger in history or get shared by accident.
4. As an operator, I want to see which server and tenant I'm looking at, so that I never act on the wrong memory.
5. As an operator, I want navigation badges showing open conflicts and open questions, so that I know at a glance what needs me.
6. As a new user with an empty tenant, I want an empty state that shows how to connect an agent (the MCP snippet), so that I know what to do next.

### Seeing claims

7. As an operator, I want to search claims by text, subject and predicate, so that I can find what an agent believes about something.
8. As an operator, I want to filter claims by scope, status, trusted, stale and source kind, so that I can focus on what matters, for example every proposed agent claim.
9. As an operator, I want every claim's status shown with a consistent word, icon and colour, so that proposed, verified, contested, superseded, retracted and quarantined are never confused.
10. As an operator, I want trusted and stale shown as separate markers from status, so that a verified-but-stale claim stands out.
11. As an operator, I want session-scoped claims hidden unless I ask for them, so that scratch work doesn't drown out durable memory.
12. As an operator, I want lists to page smoothly through thousands of claims, so that large tenants stay usable.
13. As an operator, I want a stable link for every claim, conflict, question and journal scope, so that I can share exactly what I'm looking at.

### Understanding a claim

14. As an operator, I want a claim's detail to show its subject, predicate, value, text, scope, source, actor and channel, so that I know exactly what was claimed and by whom.
15. As an operator, I want the trust score broken into its components (source prior, attestation, corroboration, contradiction, staleness), so that I can see why it is or isn't trusted.
16. As an operator, I want a plain-language explanation of the trust score ("verified by the system of record 38 minutes ago"), so that I don't have to read the maths.
17. As an operator, I want the evidence list with each item's kind, verifier, outcome, observed time and hash, and a preview of the observed payload, so that I can check what backs the claim.
18. As an operator, I want a timeline of the claim's version chain over valid time, with the current version highlighted, so that I can see how the fact changed.
19. As an operator, I want to switch the timeline between valid time and recorded time, so that I can see both when something was true and when Ledger learned it.
20. As an operator, I want to click any version on the timeline to open it, so that I can move through history.
21. As an operator, I want the journal entries that touched the claim, including the rule and policy version behind each resolution, so that I can audit every change.
22. As an operator, I want to see when a claim is shadowed by, or shadows, a claim in another scope, so that overrides are never hidden.
23. As an operator, I want a quarantined claim to show why it was quarantined, so that I can judge whether releasing it is safe.

### Changing memory

24. As an operator, I want to correct a claim by giving the right value and a reason, so that the wrong value is superseded and history is kept.
25. As an operator, I want the correction form to validate the value against the predicate's declared type, so that I can't record a malformed correction.
26. As an operator, I want to retract a claim with a reason, so that agents stop using it.
27. As an operator, I want to re-verify a claim now and watch the result arrive, so that I can refresh a stale fact.
28. As an operator, I want forgetting to require a confirmation that spells out that it can't be undone, and typing the subject, so that I never forget something by accident.
29. As an operator, I want to release a quarantined claim as my own statement, so that a false positive can enter memory deliberately.
30. As an operator, I want every action to show what it changed (the new claim, the status changes, the journal entry), so that I can confirm it did what I meant.

### Conflicts and questions

31. As a reviewer, I want a queue of open conflict sets, so that I can work through disagreements.
32. As a reviewer, I want each conflict shown side by side with every member's value, source, evidence and trust, and the rule that failed to decide, so that I can judge it.
33. As a reviewer, I want to pick the winner of a conflict set with a reason, so that the conflict closes and agents move on.
34. As a reviewer, I want an inbox of open questions (human-verifier confirmations and escalations), so that I can answer what machines couldn't.
35. As a reviewer, I want to confirm, reject, pick a winner or supply a value when answering, so that each kind of question has the right answer form.

### Journal and policy

36. As a security engineer, I want each scope's journal listed in sequence with kind, actor, channel and hashes, so that I can audit everything that happened.
37. As a security engineer, I want a chain-verification badge per scope and a button to re-run verification, so that I can see that memory hasn't been tampered with.
38. As a security engineer, I want a broken chain to point to the first broken entry, so that I can investigate exactly where it failed.
39. As a team lead, I want to read the active policy and its version history, so that I know which rules memory follows.
40. As a team lead, I want to see a diff between two policy versions, so that I can see what a change did.
41. As a team lead, I want the undeclared predicates in use, with counts and the nearest declared predicate, so that I know what to add to our policy.
42. As an operator, I want suggested subject aliases listed, so that I can decide which to declare in policy.

### Backup and restore

43. As an operator, I want to download a backup of the tenant as one file, with a summary of what it contains, so that I can keep or move memory.
44. As an operator, I want to restore a backup into an empty tenant by uploading it, confirming by typing the tenant name, so that I can recover memory deliberately.
45. As an operator, I want restore to show chain verification and then a report (scopes, entries, claims, forgets re-applied, warnings), so that I know exactly what came back.
46. As an operator, I want a restore into a non-empty tenant refused with an explanation, so that I understand why and what to do instead.

### Quality

47. As an operator, I want the visualizer to follow my system's light or dark theme, with a manual toggle, so that it's comfortable to use.
48. As an operator using a keyboard or a screen reader, I want every view and action reachable and labelled, meeting WCAG 2.2 AA, so that the tool is usable by everyone.
49. As an operator on a narrow window, I want layouts that stay readable down to phone width, so that I can check memory from anywhere.
50. As an operator, I want errors shown with their code and a plain explanation (for example, missing permission), so that I know whether to retry or ask for access.
51. As an operator, I want counts and open items to refresh on their own while the tab is visible, so that I'm not looking at stale review queues.
52. As a contributor, I want to run the visualizer with hot reload against a running `ledgermem serve`, so that UI work is fast.

## Implementation Decisions

- **Stack**: React with TypeScript, built by Vite, compiled into the `ledgermem` binary and served under `/ui` with a single-page fallback (ADR-0007). Routes are deep-linkable: claims, claim detail, conflicts, conflict detail, questions, journal per scope, policy, backup & restore.
- **Layout: Explorer**, chosen from the prototype. A header holds the wordmark, the tenant and server, the main navigation with open-conflict and open-question counts, and the theme toggle. Below it sit three panes:
  - **Scopes and filters**: scope list with counts, status filters, the session-claims toggle.
  - **Claim list**: search box, then rows showing subject, predicate, value in the claim voice, status and markers, trust score and age.
  - **Claim detail**: the claim text, then status and markers, actions, shadowing notes, the trust explanation and receipt, evidence, the version-chain timeline, claim facts, and journal entries.

  The conflicts, questions, journal, policy and backup views reuse the same header and detail conventions. Below 960 px the panes collapse to a single column, with the list first and the detail behind a "Back to claims" button.
- **Data access**: only the public REST API. A typed client is generated at build time from the server's OpenAPI document, so a contract change breaks the web build instead of the running app. Server state uses a query cache with polling (open counts every 10 seconds and on window focus, paused while the tab is hidden).
- **Sign-in**: the operator key arrives in the URL fragment of the link `ledgermem serve` prints, or is pasted. It is moved to session storage and the fragment cleared. Every request sends it as a bearer token. There is no other authentication in v1.
- **Writes**:
  - Correct is `remember` with `supersedes`, through an operator key, which the server records as a user-stated claim with human-confirmation evidence.
  - Retract and settle use `resolve`. Forget uses `forget`. Re-verify enqueues a job, and the view polls its status.
  - Release and answer use their own endpoints.
  - Backup streams a bundle download. Restore uploads a bundle.
  - Nothing in the visualizer edits or deletes in place (ADR-0003).
- **Destructive confirmations**: forget requires typing the subject, and restore requires typing the tenant id. Both state that they can't be undone.
- **Design system**: a small set of in-house components on CSS custom-property tokens. No third-party component framework.
  - **Palette (light)**: white and violet, taken from the author's portfolio site. Page `#F5F5F7`, panels `#FFFFFF`, borders `#E5E5EA`, text `#1D1D1F` / `#515154` / `#6E6E73`, accent `#7C3AED` (strong `#6D28D9`, soft `#F3EEFE`), selection `#DDD0FD`.
  - **Palette (dark)**: a standard neutral-grey dark mode. Page `#1C1C1E`, panels `#2C2C2E`, borders `#3A3A3C`, text `#F5F5F7` / `#C7C7CC` / `#98989D`. The accent lifts to `#A78BFA` for text and keeps `#7C3AED` for filled buttons.
  - The theme follows the system setting, with a manual toggle.
  - **Accent is for interaction only**: the selected row, the current navigation item, primary buttons, focus rings, and the double rules under the wordmark and the trust total.
  - **Status colours never use the accent**: proposed amber, verified green, contested blue, superseded grey, retracted red, quarantined raspberry, each with light and dark values. Status is always word + icon + colour, never colour alone.
  - **Type voices**: claim text (what was said) in Source Serif 4; evidence, hashes and ids (what was observed) in JetBrains Mono; the interface in Atkinson Hyperlegible Next.
  - **The trust receipt**: trust is shown as an itemised breakdown with a double-ruled total, the visualizer's signature element.
- **Timeline**: a hand-built SVG component driven by a pure layout function. The function takes a version chain and a time axis (valid or recorded) and returns positioned segments, markers and lanes. Keeping layout pure makes it testable and keeps chart libraries out of the bundle.
- **Trust explanation**: a pure function from trust components and evidence to one plain-language sentence, shared by claim detail and the conflict view.
- **Budgets**: initial JavaScript under 250 KB gzipped. Lists use cursor pagination from the API. The first view renders in under a second against a local server.
- **Build integration**: the server crate embeds the built assets. Release packages include prebuilt assets, so `cargo install` needs no Node. A Rust-only build without assets serves a page explaining how to build the UI.

## Testing Decisions

- **What makes a good test here**: it drives the visualizer the way an operator does, through the browser, against a real server, and asserts on what the operator sees and on the resulting memory state. It never asserts on component internals or rendered markup structure.
- **End-to-end (primary seam)**: Playwright runs against a real `ledgermem serve` on a temporary database loaded by `ledgermem demo`. It covers each story group:
  - signing in from the printed link
  - filtering claims
  - reading a claim's trust breakdown and timeline
  - correcting, then confirming the superseded version on the timeline
  - retracting, forgetting with confirmation, and releasing from quarantine
  - settling a conflict and answering a question
  - the journal badge, including a deliberately broken chain fixture
  - a backup download, and restore into a fresh tenant, comparing claim counts
  - refusal of a non-empty restore

  Automated accessibility checks run on every view in the same suite.
- **Unit tests only for pure functions**: the timeline layout function, the trust explanation function, and the mapping from filters to query strings. No component snapshot tests.
- **Contract safety**: the typed client is regenerated from the OpenAPI document in CI, and the web build fails on drift.
- **Prior art**: none in this repository yet. The core spec's demo dataset is the shared fixture.

## Out of Scope

- A graph view of subjects and their relationships (planned next).
- Alternative views explored in the prototype: a timeline-first "Chronicle" view and a review-queue-first "Desk" view. The Explorer ships first, and these may come later as extra views.
- Editing the policy in the browser (read-only in v1; apply through the CLI or API).
- Managing tenants and API keys (CLI only in v1).
- Switching between tenants (one tenant per operator key).
- Push updates over server-sent events or websockets (polling in v1).
- OAuth or SSO sign-in.
- Internationalisation.
- Bulk actions across many claims.

## Further Notes

- The REST endpoints used are listed in `docs/design/contracts.md`, sections 2 and 3. The views map to them one-to-one.
- A throwaway UI prototype answered "what should the visualizer look like?" with three layouts: Explorer, Chronicle and Desk. **Verdict: Explorer**, restyled in the portfolio palette with a grey dark mode. Two findings carry into the real build:
  - A linear time axis crushes recent activity, so any timeline needs a focus-plus-context scale.
  - Receipts inside narrow containers should drop their explanation column.

  The prototype lives on the `prototype/visualizer` branch, and the visualizer tickets point to it. Its findings fold into the real components. Its code does not.
