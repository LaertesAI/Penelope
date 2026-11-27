# Workflows

Stand 27.11.2026. Teil C aus `Auftrag Teil C und D.md`. Ein Workflow ist ein kleines Skript, das mehrere Agenten deterministisch steuert. Das Skript läuft in einer Sandbox ohne Netz und ohne Dateizugriff. Jeder Agent läuft mit Eurykleia, den Kerkis-Gates und dem Sparmodus.

## C1: Sprachwahl

Verglichen: Rhai, mlua (Lua 5.4), rquickjs (QuickJS) und Boa. Die Zahlen sind aus zwei unabhängigen Messungen und den Herstellerangaben. Alle Werte sind markiert.

| Runtime | Binär gestrippt | Idle RSS | Start | Windows | Sandbox | Async |
|---|---|---|---|---|---|---|
| Rhai 1.27 | 1,29 MiB (2), +2,6 MiB (1) | 2,88 MiB (1) | unter 1 ms | pure Rust | Standard verweigert, Ops, Tiefe, String, Array | nein |
| mlua 0.12, Lua 5.4 | 0,58 MiB (2), +416 KiB (1) | 1,95 MiB (1) | unter 1 ms | vendored C, getestet (5) | Standard verweigert | Feature |
| rquickjs 0.14 | 0,85 MiB (2), +1,0 MiB (1) | 2,16 MiB (1) | unter 300 us (QuickJS, 5) | MSVC experimentell (5) | Standard verweigert | ja |
| Boa | nicht im Fiber-Lauf, größer als Rhai (4) | nicht gemessen | nicht gemessen | pure Rust | Standard verweigert | nein |

Quellen:
1. Fiber, Embeddable scripting runtimes for Rust, 21.09.2026, macOS arm64. Gestrippte Binärdatei und Idle-RSS, gleiche Basis.
2. Quirl, embedded-language-decision, 15.08.2026. Gestrippte Binärdatei, eigener Vergleich.
3. Rhai Book und crates.io, 1.27.0. Sandbox, Ops-Grenzen, 1 Mio. Iterationen in 0,14 s.
4. khvzak/script-bench-rs, Boa ist enthalten, Rhai ist kleiner und schneller.
5. rquickjs crates.io, 0.14.0. Windows MSVC ist als experimentell markiert. mlua testet Windows in der CI.

### Entscheidung: Rhai

Gründe:

- **Windows-Build ohne C.** Rhai ist pure Rust. mlua und rquickjs brauchen einen C-Compiler, rquickjs ist auf MSVC experimentell. Die CI auf Windows und macOS ist Pflicht. Rhai ist der einzige Kandidat ohne Toolchain-Risiko.
- **Sandbox mit Grenzen.** Rhai verweigert Datei, Netz und Prozess von sich aus. `set_max_operations`, `set_max_expr_depths`, `set_max_string_size`, `set_max_array_size` und `set_max_map_size` begrenzen den Lauf. Das ist genau der Budgethebel aus C3.
- **Kein Async.** Die Workflow-Funktionen sind synchron. `parallel` und `pipeline` laufen im Host in Threads, nicht im Skript. Rhai braucht kein Future.
- **Klein und schnell.** 1,29 bis 2,6 MiB Binärdatei, unter 1 ms Start, 2,88 MiB Idle. Lua ist 0,7 MiB kleiner, rquickjs 0,4 MiB. Der Unterschied ist für Penelope unwichtig, das Risiko auf Windows nicht.
- **Die Sprache reicht.** Arrays, Maps, Funktionen, Closures, Schleifen, `throw`, `try`/`catch`. Die Workflow-Beispiele brauchen nichts darüber hinaus.

Die lokale Messung der Penelope-Binärdatei mit Rhai folgt nach C5. Die Entscheidung ändert sich nur, wenn die lokale Binärdatei über 20 MiB wächst oder der Start über 50 ms liegt.

## C2: Funktionen im Skript

| Funktion | Verhalten |
|---|---|
| `agent(prompt, opts)` | Startet einen Teil. `opts`: `model` (Rolle oder Name), `effort`, `schema` (JSON-Schema, Antwort geprüft und als Map zurück), `label`, `phase`, `worktree` (eigener Git-Worktree). |
| `parallel([...])` | Startet alle Einträge und wartet auf alle. Jeder Eintrag ist eine Map mit `prompt` und `opts`. |
| `pipeline(items, stage1, stage2, ...)` | Schickt jedes Element durch die Stufen. Stufe 2 eines Elements startet, sobald Stufe 1 fertig ist, ohne Sperre für andere Elemente. |
| `phase(title)` | Setzt die Phase im Journal und in der Baumansicht. |
| `log(text)` | Schreibt eine Zeile ins Journal. |
| `args` | Die Argumente des Laufs als Map. |
| `budget` | Die Grenze für gerechnete Kosten und Tokens. Wird sie überschritten, stoppt der Lauf. |
| `workflow(name, args)` | Ruft einen gespeicherten Workflow auf, eine Ebene tief. |
| `loop_until(condition, step, {max_iterations})` | Wiederholt den Schritt, bis die Bedingung wahr ist oder die maximale Zahl an Runden erreicht ist. Jede Runde steht im Journal mit Grund. |
| `ask(questions)` | Stellt dem Nutzer mehrere Fragen. Feste Antworten, Mehrfachauswahl, freie Antwort. Dieselbe Logik wie das Rückfragefenster. |

## C3: Ablauf und Sicherheit

- Höchstens `workflow.max_parallel` Agenten. Voreinstellung aus RAM und CPU. Auf dem Rechner des Owners ist der RAM knapp, deshalb höchstens 4 und höchstens ein Viertel der freien GB.
- Vor dem Start zeigt Penelope den Plan: Zahl der Agenten, Modelle, Budget. Ohne Ja des Nutzers startet nichts.
- Jeder Agent läuft mit Eurykleia, den Kerkis-Gates und `route: auto`.
- Journal je Lauf in `.penelope/workflows/<id>/journal.jsonl`. Ein abgeschlossener Aufruf mit gleichem Prompt und gleichen Optionen kommt aus dem Journal, nicht aus dem Modell.
- Kein Agent mergt, pusht nach `main` oder installiert etwas. Der Workflow setzt `run_policy` und `route` so, dass diese Werkzeuge verweigert werden.
- Builds laufen über `flock ~/.cache/penelope-cargo.lock`.

## C4: Bedienung

- `/workflow run NAME|DATEI`, `/workflow list`, `/workflow status ID`, `/workflow stop ID`.
- Die laufenden Agenten als Baum, mit Phase, Modell, Kosten und Status.
- Gespeicherte Workflows in `.penelope/workflows/*.rhai`.
- Beispiele: `review.rhai` (Review mit Prüfung jedes Befunds), `build.rhai` (Bauen mit Rechenprüfung und Review), `research.rhai` (Recherche mit Annahmen-Register).

## C5: Tests

- Sandbox: kein Dateizugriff, kein Netz, kein Prozess aus dem Skript.
- Schema: eine Antwort, die nicht zum Schema passt, wird abgelehnt.
- `parallel` und `pipeline` mit einem falschen Modell.
- Budgetgrenze: der Lauf stoppt vor dem nächsten Agenten.
- Journal: ein abgeschlossener Aufruf wird nach einem Absturz nicht wiederholt.
- Abbruch: `stop` beendet die laufenden Agenten.
- Bestätigung: ohne Ja startet kein Agent.

## C6: Messung

Derselbe Review-Workflow einmal mit 3 Agenten und einmal mit 1 Agenten auf 10 bekannten Fehlern in einem Testrepo. Treffer, gerechnete Kosten, Zeit. Die Vorhersage steht vor dem Lauf in `predictions.md`. Abbruch: die 3-Agenten-Variante findet nicht mehr Fehler oder braucht über 1,2-mal die Zeit.

Die Messung startet erst nach C1 bis C5 und nach der A2-Hold-out. Kein bezahlter API-Lauf, nur das Claude-Abo.
