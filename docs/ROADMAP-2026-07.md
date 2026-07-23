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
| 6 | Mock-Server + Response-Examples | reqv1 CLI fertig; eigener GUI-Server-Schalter weiterhin zurückgestellt | |

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

## Request-Format v1 und Katalog

- Built-in-Katalog, typisierte Parameterformulare, Intent-Suche und lokale
  Vorschau sind umgesetzt.
- Projekt-Assets können ihre Formularmetadaten als `<asset>.meta.json`
  direkt neben der `.js`-Datei ablegen.
- Matrixfälle und gespeicherte Request-Sequenzen laufen im zentralen
  v1-Editor.
- Projekt-JavaScript benötigt in GUI und CLI eine explizite Freigabe.
- `apiwright migrate` konvertiert den verlustfrei darstellbaren Teil eines
  Legacy-Requests und bricht bei nicht abbildbaren Feldern ab.

Arbeitsweise: ein Feature pro Commit, Tests + clippy grün vor jedem Commit,
Verifikation end-to-end (CLI/Engine gegen lokalen Testserver, GUI unter Xvfb).
