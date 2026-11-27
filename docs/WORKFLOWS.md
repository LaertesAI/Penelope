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

Die lokale Messung, 27.11.2026, Release-Build mit `--release`:

| Messung | Wert |
|---|---|
| Binärdatei | 17.105.552 Byte = 16,3 MiB (gestrippt, `strip` änderte nichts) |
| Start `penelope --version`, Median aus 20 Läufen | 1,0 ms (min 0,9, max 1,5) |
| Max RSS `penelope --version` | 13.736 KiB = 13,4 MiB |
| Bauzeit Release, 2 Jobs | 1 min 58 s |

Die Entscheidung bleibt. Die Binärdatei liegt unter der Marke von 20 MiB, der Start weit unter 50 ms. Rhai kostet nach der Literatur 1,29 bis 2,6 MiB; die Basis von Penelope ist der große Teil.

## C2: Funktionen im Skript

| Funktion | Verhalten |
|---|---|
| `agent(prompt, opts)` | Startet einen Teil. `opts`: `model` (Rolle oder Name), `effort`, `schema` (JSON-Schema als Map oder JSON-Text, Antwort geprüft und als Map zurück), `label`, `phase`, `worktree` (eigener Git-Worktree, bleibt nach dem Lauf stehen und steht mit Pfad und Zweig im Journal). |
| `parallel([...])` | Startet alle Einträge und wartet auf alle. Jeder Eintrag ist eine Map mit `prompt` und `opts`, `schema` wird je Eintrag geprüft. Höchstens `max_parallel` Einträge. Kein `worktree` innerhalb von `parallel`. |
| `pipeline(items, stage1, stage2, ...)` | Schickt jedes Element durch die Stufen. Stufe 2 eines Elements startet, sobald Stufe 1 fertig ist, ohne Sperre für andere Elemente. Jede Stufe hat einen Journal-Schlüssel und wird nach einem Absturz aus dem Journal bedient. |
| `phase(title)` | Setzt die Phase im Journal und in der Baumansicht. |
| `log(text)` | Schreibt eine Zeile ins Journal. |
| `args` | Die Argumente des Laufs als Map. |
| `budget` | Die Grenze für gerechnete Kosten und Tokens. Wird sie überschritten, stoppt der Lauf vor dem nächsten Agenten. |
| `workflow(name, args)` | Ruft einen gespeicherten Workflow auf, eine Ebene tief. Das Restbudget geht mit, die Kosten des inneren Laufs zählen zum äußeren. |
| `loop_until(condition, step, {max_iterations})` | Wiederholt den Schritt, bis die Bedingung wahr ist oder die maximale Zahl an Runden erreicht ist. Jede Runde steht im Journal mit Grund. |
| `ask(questions)` | Stellt dem Nutzer mehrere Fragen. Feste Antworten, Mehrfachauswahl, freie Antwort. Dieselbe Logik wie das Rückfragefenster. |

Der Journal-Schlüssel ist FNV-1a über Prompt und Optionen. `agent` und `parallel` tragen ihn je Aufruf ein, `pipeline` je Stufe. Ein abgeschlossener Aufruf mit gleichem Prompt und gleichen Optionen kommt aus dem Journal. Der Stop-Marker `.penelope/workflows/<id>/stop` wird vor jedem Agenten, vor jedem Parallelblock und vor jeder Loop-Runde geprüft; `/workflow stop ID` wirkt damit auch aus einem zweiten Prozess.

## C2b: Grenzen aus der Klärung

Ein Cluster liest `clusters/ID/spec.json` vor dem ersten Agenten. Nur ein bestätigter Spec (`confirmed: true`) ändert das Profil. Abgebildet werden:

| Spec-Feld | Profilfeld | Beispiel |
|---|---|---|
| `budget` | `budget_eur` | `"5 EUR"`, `"1,5 EUR"`, `"20 €"` |
| `max_iterations` | `max_steps` (1 bis 30) | `"10 iterations"` |
| `abort` | `stuck_after` (1 bis 100) | `"Stuck for 3 reports"`, `"first failed check"` wird 1, `"never"` wird 100 |

Ein Token-Budget ändert `budget_eur` nicht. Ein Entwurf ohne `confirmed` ändert nichts. Die Funktion ist `apply_spec_limits` in `clusters.rs`, mit Test.

Eine Factory mit `approve_each_run: yes` läuft über `penelope factory run NAME`. Der Timer startet diesen Befehl, der Befehl legt die Frage in `.penelope/approvals` ab und wartet auf ein Ja aus der laufenden Oberfläche. Kommt keins, wird der Lauf übersprungen und kein Modell gestartet. Der Timer allein startet damit nie einen Lauf. Test: `a_stored_approval_waits_for_a_yes_and_the_list_shows_the_spec`.

## C3: Ablauf und Sicherheit

- Höchstens `workflow.max_parallel` Agenten. Voreinstellung aus der Maschine: halbe CPU-Zahl, höchstens 4, und höchstens ein Agent je 1,5 GB freiem Speicher, mindestens 1. Auf dem Rechner des Owners ist der Speicher die Grenze.
- Vor dem Start zeigt Penelope den Plan: Zahl der Agenten, Loop-Schritte, Budget, Skript. Ohne Ja des Nutzers startet nichts. In der CLI fragt `penelope workflow run NAME`; in der TUI zeigt `/workflow run NAME` den Plan und startet erst mit `/workflow run NAME yes`.
- Jeder Agent läuft mit Eurykleia, den Kerkis-Gates und `route: auto`, soweit gemergt.
- Der Workflow setzt `run_policy` auf `safe`, auch wenn die Sitzung `all` erlaubt. `git push`, `git merge` und Paketinstallationen lösen damit eine Frage aus, die ein Workflow nicht beantworten kann.
- Journal je Lauf in `.penelope/workflows/<id>/journal.jsonl`. Fortsetzen nach Absturz: abgeschlossene Aufrufe mit gleichem Prompt kommen aus dem Journal. `parallel` und jede `pipeline`-Stufe haben eigene Schlüssel.
- Kein Agent mergt, pusht nach `main` oder installiert etwas.
- Builds laufen über `flock ~/.cache/penelope-cargo.lock`.

Grenzen: `worktree` gilt nur für `agent`, nicht für Einträge in `parallel`. Ein verschachtelter Workflow kann nicht `ask` aufrufen; das äußere Skript fragt. Worktrees bleiben nach dem Lauf stehen, damit ein Bau-Workflow sein Ergebnis behält; `penelope work list` zeigt sie.

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
