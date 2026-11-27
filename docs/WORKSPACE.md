# Penelope workspace: core and command line

Stand 27.11.2026. Der Umbau für Penelope Desktop. D1 aus `intern/Penelope Desktop/Plan.md`.

## Layout

```
Cargo.toml          # [workspace] members = ["core", "cli"]
Cargo.lock
core/               # penelope-core, eine Bibliothek ohne Benutzeroberfläche
  src/lib.rs
  src/api.rs        # die JSON-Fläche für einen Desktop
  src/theme.rs      # Farben eines Mods als Daten
  src/ask.rs        # Rückfragen: Daten, Dialog-Zustand und Dateiprotokoll
  src/doctor.rs     # eine Zeile je Programm oder Einstellung
  src/<modul>.rs    # die gemeinsame Logik
cli/                # penelope, das Binary mit ratatui
  src/main.rs
  src/tui/
  src/input.rs
  src/update.rs
```

Der Kernel liegt in `penelope-core`. Die CLI und ein Desktop sprechen mit derselben Bibliothek. Der Kernel importiert kein `ratatui`, kein `crossterm` und kein `tauri`.

## Was im Kern liegt

| Gruppe | Module |
|---|---|
| Zustand und Config | `config`, `sessions`, `cost`, `route`, `autoroute`, `mode`, `weak`, `startup`, `setup`, `doctor`, `theme` |
| Modelle | `providers`, `llm`, `backend`, `auth`, `crew`, `spec`, `pii`, `privacy_bench` |
| Werkzeuge | `tools`, `mcp`, `mcpclient`, `mcpmem`, `mcpserv`, `mcpvet`, `toolscan`, `mitos`, `attach`, `eurykleia`, `proof`, `files`, `work`, `search`, `scrape`, `net`, `lsp`, `skills`, `toolsearch`, `deepresearch` |
| Agenten | `clusters`, `agents`, `advise`, `clarify`, `notes`, `memory`, `graph`, `context`, `brain`, `scout`, `research`, `loom`, `forge`, `audit` |
| Aufgaben | `eval`, `brief`, `jobs`, `factories`, `apps`, `mods`, `plugins`, `vet`, `checker`, `gates` |
| Gerät | `screen`, `cursor`, `privacy`, `platform` |
| Server | `server`, `acp` |
| Rückfragen | `ask` |
| Desktop-Fläche | `api` |

`workflows` folgt aus Teil C und kommt in denselben Kern, sobald der Zweig gemergt ist. Bis dahin kennt der Kern keinen Workflow-Lauf.

## Was in der CLI bleibt

| Modul | Grund |
|---|---|
| `main` | Start, Kommandozeile, Unterbefehle. |
| `tui` | Ratatui-Oberfläche, Panels, Tasten. |
| `input` | der uinput-Helfer für Wayland. Er baut auf `platform::input_binary` auf. |
| `update` | Selbstupdate der CLI. |

## Grenze

Der Kern gibt Serde-Typen und JSON aus. `backend::Event` trägt `#[derive(Serialize)]` und geht als JSON über einen Kanal. Die CLI zeichnet dieselben Events mit ratatui. Der Desktop zeichnet sie im Webview.

`api` ist die kleine Fläche für ein Fenster:

- `root`, `load`: eine `Ctx` laden.
- `chat`: ein Zug über `backend::Chat`.
- `event_json`: ein Event als JSON.
- `provider_rows`, `model_rows`, `set_provider`, `save_key`, `remove_key`, `key_rows`.
- `settings_json`, `save_settings`.
- `usage_rows`, `costs_json`, `route_state`.
- `clusters_json`, `cluster_detail`.
- `eval_runs`, `last_eval`.
- `attachments_json`.

Ein Schlüssel wird nie zurückgegeben. `save_key` schreibt in den Secret Service oder in `~/.penelope/auth.json` mit Modus 0600. Die Antwort nennt nur den Ort.

## Rückfragen

`ask.rs` ist die gemeinsame Logik für `ask_user` mit 1 bis 6 Fragen. Es hält feste Antworten, Mehrfachauswahl und freie Antwort, den Dialog-Zustand ohne Zeichencode und das Dateiprotokoll. Ein Lauf schreibt eine `.req`-Datei in `PENELOPE_ASK_DIR` oder `PENELOPE_APPROVE_DIR` und wartet auf `.ans`. Die CLI, der Desktop und der ACP-Server können denselben Dialog zeigen. Der Dateiinhalt ist JSON; ein Test prüft den Weg hin und zurück.

## Bauen und prüfen

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release -p penelope-cli
```

Das Binary liegt dann unter `target/release/penelope`.

## Desktop

Das private Repo `LaertesAI/Penelope-Desktop` hängt am Zweig `feat/desktop-core` dieses Repos:

```toml
penelope-core = { git = "https://github.com/LaertesAI/Penelope.git", branch = "feat/desktop-core" }
```

Der Zweig wird nicht selbst gemergt. Nach der Freigabe des Owners zeigt die Abhängigkeit auf `main`.
