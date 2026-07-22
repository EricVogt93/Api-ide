# Licensing and billing

ApiWright has three plans. The full legal terms live in
[LICENSE](https://github.com/EricVogt93/apiwright/blob/development/LICENSE) and
[COMMERCIAL-LICENSE.md](https://github.com/EricVogt93/apiwright/blob/development/COMMERCIAL-LICENSE.md).

| Plan | Price | For |
|------|-------|-----|
| Free | 0 € | Personal and other noncommercial use (PolyForm Noncommercial 1.0.0). No key, no account. |
| Pro | 12 € per user / month | Any commercial use. Billed monthly, cancel anytime. |
| Enterprise | Custom | Commercial use with a license server hosted in your own infrastructure, volume pricing and invoicing. |

## Pro features

Some team-oriented features require a Pro or Enterprise license (or an
active commercial trial). Currently: the Jira integration (link editing,
ticket details, comments) and the ticket/OpenAPI coverage report —
existing Jira links stay visible on every plan. More team features will
follow the same rule: solo, noncommercial work stays fully functional on
Free.

ApiWright is open core: the Pro engine lives in a private `forge-pro` crate
that only official release builds link in (`--features pro`). Building
this repository yourself always produces the Free edition; the public
code compiles without the private crate.

## 60-day commercial trial

Commercial teams can evaluate ApiWright on the Free plan for 60 days. Open
**Help → License & Billing** and click **Start 60-day commercial trial** —
no key, no account, nothing leaves the machine. The dialog shows the
remaining days; after expiry, commercial use requires a Pro or Enterprise
license.

## Activating a license

1. Open **Help → License & Billing**.
2. Paste the license key.
3. Enterprise only: enter the base URL of your own license server; Pro keys
   use the hosted server automatically.
4. Click **Activate**.

The key is validated online once and the verdict is cached in
`~/.config/forge/license.json`. ApiWright re-checks in the background at most
once a day; if the license server is unreachable the cached license keeps
working until the end of the already-paid period, so offline work is never
interrupted mid-subscription.

## Free and commercial use

The Free plan is not enforced technically — ApiWright sends no telemetry and
does not try to detect where it runs. Using ApiWright for a commercial purpose
without a paid license is a violation of the license terms, not a technical
impossibility.
