# Jira integration

ApiWright ties API tests to the tickets they belong to — and talks to Jira
directly. The integration is a ApiWright Pro feature (included in the free
60-day commercial trial, see [Licensing and billing](licensing.md));
plain links stay visible on every plan.

## Linking tickets

Right-click a story folder or a single request → **Link Jira ticket…**.
Children inherit the folder's link, one link at the story level covers
every request underneath. Links live in plain `.forge-jira` files next to
the nodes they annotate, so they are reviewable in pull requests and
travel with exported bundles.

## Connecting to Jira

**Settings → Jira**:

| Field | Jira Cloud | Jira Server / Data Center |
|-------|------------|---------------------------|
| Base URL | `https://yourcompany.atlassian.net` | your Jira URL |
| Email | account email | leave empty |
| API token | [API token](https://id.atlassian.com/manage-profile/security/api-tokens) | personal access token |

The connection is stored per user in `~/.config/forge/jira.json`
(owner-readable only) — never in the project, never in Git.

## Ticket details and comments

Right-click a linked node → **Ticket details…** fetches summary, status,
type and assignee live from Jira. From the same dialog you can open the
ticket in the browser or post a comment (for example a run summary) without
leaving ApiWright.

The issue key is extracted from the link automatically — both bare keys
(`SHOP-42`) and full URLs (`https://…/browse/SHOP-42`) work.

## Coverage report

**Run → Coverage report…** joins three axes over the execution history:

- **Ticket → tests** — every test grouped under its (inherited) Jira link.
- **Tests → OpenAPI** — which spec operations the tests cover, plus the
  list of operations no test touches.
- **History → health** — per test over the last 50 runs: pass rate,
  median/p95/max runtime, **flaky detection** (pass↔fail flips) and
  **hiccups** (transport errors plus runs slower than 3× the test's
  median).

Flaky and failing tests sort to the top of each section. Export the report
as Markdown or JSON, or post a ticket's section straight into Jira as a
comment ("Comment report to SHOP-42…").

The same report runs headless for CI:

```
apiwright report <project-root> [--ticket SHOP-42] [--json] [--out report.md]
```

Verdicts (assertions passed/failed) are recorded into the run history as
tests execute; history from older ApiWright versions is judged by HTTP status
as a fallback.
