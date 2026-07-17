# Roadmap — Arbeitsstand 2026-07-17

Auftrag: fehlende Features für eine vollwertige API-IDE. Kein Postman-Export;
stattdessen Import von Postman (fertig) und Bruno.

| # | Feature | Status | Commit |
|---|---------|--------|--------|
| 1 | mTLS-Client-Zertifikate + Custom-CA | fertig | 82eab03 |
| 2 | Bruno-Collection-Import (.bru) | fertig | 197198a |
| 3 | pm.\*-Shim (Postman-Scripts lauffähig) | fertig | 429ce0d |
| 4 | Digest- + AWS-SigV4-Auth | fertig | 7eccd19 |
| 5 | gRPC (unary, dynamische .proto) | fertig | ff87872 |
| 6 | Mock-Server + Response-Examples | zurückgestellt (Erinnerung 2026-08-17) | |

## Nachgezogene Lücken (2. Runde)

Die ursprünglich als "bewusste Auslassungen" dokumentierten Punkte sind
jetzt ebenfalls geschlossen:

| Lücke | Status | Commit |
|-------|--------|--------|
| pm.sendRequest (war: "wirft klaren Fehler") | fertig | 7f8c082 |
| gRPC-Streaming server/client/bidi (war: abgelehnt) | fertig | 16f7675 |
| mTLS/Custom-CA für WebSocket + SSE | fertig | 88ad320 |
| NTLM-Auth (war: "kein tragfähiger Weg") | fertig | (dieser Commit) |

- **NTLM**: über die `ntlmclient`-Crate (reine Rust-NTLMv2-Berechnung),
  nicht über SSPI/rustls — der Handshake läuft auf HTTP/1.1-Ebene über eine
  gepinnte Keep-alive-Connection, unabhängig vom TLS-Stack. Gegen einen
  Testserver verifiziert, der den NTLMv2-Proof eigenständig nachrechnet.
- **Postman-Export**: weiterhin explizit nicht gewünscht.

Arbeitsweise: ein Feature pro Commit, Tests + clippy grün vor jedem Commit,
Verifikation end-to-end (CLI/Engine gegen lokalen Testserver, GUI unter Xvfb).
