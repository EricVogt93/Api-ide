# Roadmap — Arbeitsstand 2026-07-17

Auftrag: fehlende Features für eine vollwertige API-IDE. Kein Postman-Export;
stattdessen Import von Postman (fertig) und Bruno.

| # | Feature | Status | Commit |
|---|---------|--------|--------|
| 1 | mTLS-Client-Zertifikate + Custom-CA | offen | |
| 2 | Bruno-Collection-Import (.bru) | offen | |
| 3 | pm.\*-Shim (Postman-Scripts lauffähig) | offen | |
| 4 | Digest- + AWS-SigV4-Auth | offen | |
| 5 | gRPC (unary, dynamische .proto) | offen | |
| 6 | Mock-Server + Response-Examples | offen | |

Bewusste Auslassungen:

- **NTLM**: kein tragfähiger Weg auf dem rustls-Stack; Import meldet es
  weiter ehrlich als "not supported".
- **Postman-Export**: explizit nicht gewünscht.

Arbeitsweise: ein Feature pro Commit, Tests + clippy grün vor jedem Commit,
Verifikation end-to-end (CLI gegen lokalen Testserver, GUI unter Xvfb).
