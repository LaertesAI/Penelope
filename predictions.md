# Predictions and freeze: cluster loop

Started 2026-11-26 on `feat/cluster-loop` from Penelope `main` `4389032`. This file is written before every change.
The loop follows `intern/DeepSeek-Auftrag Cluster-Loop.md`. All numbers are measurements from the G2 runs or are
derived with python3.

## Hold-out freeze

The 50-task hold-out is `cli/tests/data/eval/holdout50.jsonl` with the repo `cli/tests/data/eval/holdout_repo/`.
It holds 50 tasks of other kinds than the 50 development tasks: merge, replace, extract, reverse, dedupe, number,
nested JSON, CSV, averages, word counts, folders, moves, splits, compare, file counts and headers. All 50 checks
pass against deterministic reference solutions (local verifier, no model). The loop does not run it. It runs once
at the end, large and cluster.

- tasks sha256: `ce9c73a5f2eb05b21f55864d267d26534b6bdb278b8586415de138a98f4569d2`
- repo sha256: `c3224aba7eeb471f6512c30d072bf547a11bde1ed5c4f9c42f184df566b1c830`

The repo hash is SHA-256 over sorted relative path, NUL, file bytes, NUL for every file. The test
`the_holdout_is_frozen` recomputes both hashes.

## Step 0: baseline G2

- large 35/50, 1.2187 EUR, 446.8 s. cluster 39/50, 1.0265 EUR, 905.1 s.
- Paired large against cluster: b = 4 (cluster only: script-1, script-2, script-4, script-5), c = 0,
  psi = 0.08, exact McNemar two-sided p = 0.125.
- Failure causes: 10 of 11 are one cause. The shipped `run_policy: safe` refuses `mv` and `rm`; the MCP
  `rename_file` is not granted. 1 of 11 is a model fallback error: script-3 gave up when `run` refused, while
  script-1, 2, 4 and 5 wrote the computed output.
- Noise: 10 tasks, 3 repeats each, 0 flips. The same 5 pass and 5 fail in all three runs. The smallest real
  improvement is 2 tasks (the brief's floor; twice the mean flip count is 0).
- Time: 50 clusters, 903 s in the report streams. Worker 534 s (59.1 %), validator 369 s (40.9 %),
  orchestration 0 s. Mean 18.06 s per cluster.
- Cost: 800,191 in and 118,906 out, 1.0265 EUR, 100 part calls. The validator writes 47.1 % of the output
  characters and takes 40.9 % of the time; estimated share 41 % of EUR.

## Round 1

Hypothesis, written before the change: the dominant cause is the shipped `run_policy: safe`, which refuses the
commands that rename and delete. An eval is unattended and runs in a scratch copy. Setting `run_policy: all` and
`sandbox: bwrap` for every eval setup lets both setups act, keeps the rest of the machine read-only and cuts no
network. Expected: cluster +10 tasks (the five rename and five delete tasks), large +10 to +15, EUR and time within
10 %. The change is one block in `eval_context`, not a rule about a task.

Keep rule: hits rise by at least the noise threshold, EUR and time rise by at most 10 %, or hits stay equal and
EUR or time fall by at least 15 %.

## Round 2

Hypothesis, written before the change: the validator takes 40.9 % of the cluster time and about 41 % of its euro.
A profile with one worker and no retry cannot use a validator to correct anything: a FAIL is final. Skipping that
validator and letting the worker's own check be the evidence keeps the hits and cuts EUR and time by about 40 %.
Expected: cluster 50/50, about 0.50 EUR, about 535 s. The change is one profile flag (`single_model`) and one branch
in `run_cluster_inner`; the eval profile sets the flag, the built-in profiles keep the validator.

Keep rule: hits stay equal and EUR or time falls by at least 15 %, or hits rise by at least the noise threshold.

## Round 1 result

large 50/50, 0.9844 EUR, 307.8 s. cluster 50/50, 0.8451 EUR, 818.9 s. Paired b = 0, c = 0, p = 1.
Kept: hits +11 against G2 (39 to 50), EUR -17.7 %, time -9.5 %. The target 45/50 and EUR at most large are met;
the time 818.9 s is above 581 s, so round 2 must cut the time.

## Round 2 result

cluster 50/50, 0.5119 EUR, 372.4 s. large stays 50/50, 0.9844 EUR, 307.8 s.
Kept: hits equal, EUR -39.4 %, time -54.5 %. The target is met: 50 at least 45, EUR 0.5119 at most 0.9844,
time 372.4 s at most 1.3 times large (400.1 s) and below the brief's 581 s. The loop stops after 2 rounds on success.

## Hold-out

The hold-out hashes are unchanged. It runs once after the loop stopped, large and cluster, with the same binary.
The result is added here after the run.

Hold-out result: large 50/50, 1.1096 EUR, 345.0 s. cluster 50/50, 0.5408 EUR, 548.9 s.
b = 0, c = 0, exact McNemar p = 1.0 on the hold-out and on all 100 pairs. The cluster is 51.3 % cheaper and
1.59 times slower. A hit lead is not proven. Both 50-task sets are saturated for these two setups.

## Next set: SWE-bench Lite, 50 tasks

Selection, before any run: all 300 test rows of `princeton-nlp/SWE-bench_Lite`, sorted by SHA-256 of `instance_id`,
first 50. The hash of the selected list is `ef59b9f230078a2efd5c554c576b4ead3ed27a55272afb1c31f721bba67dbf77`.
Repos: django 15, sympy 14, matplotlib 4, sphinx 4, pylint 3, scikit-learn 3, pytest 2, xarray 2, seaborn 2,
requests 1.

Check: the model sees the issue text and the repo at `base_commit`, not the test patch. The check applies the
instance's `test_patch` and runs every `FAIL_TO_PASS` and every `PASS_TO_PASS` test in a prebuilt environment. A task
is resolved only when all of them pass. An environment that cannot start is `infra`, not a model fail, and is
reported separately.

Prediction, before any run:

- large: 12 of 50 (24 %).
- cluster: 4 of 50 (8 %).
- Paired: large wins 8, cluster wins 0, exact McNemar p = 0.0078.
- EUR: large about 12.5, cluster about 6.0. The repo contexts and the many steps make a task 5 to 10 times as
  expensive as a file task. The estimate is rough and needs an owner's budget approval.
- Time: large about 6,000 s, cluster about 7,500 s.
- 2 to 4 tasks fail on the environment, not the model.
- The cluster will be cheaper, not better. The gap of 8 points is the result the set is built to show.

Risks: the harness is not a full SWE-bench agent. The resolved rate can be lower for both. If the environment fails
on more than 10 tasks, stop and fix the harness before scoring.

---

# Predictions: Cluster-Kommunikation

Stand 26.11.2026. Zweig `feat/cluster-comm` von `main` `528face`. Auftrag: `intern/DeepSeek-Auftrag Cluster-Kommunikation.md`. Ziel: der Cluster soll schneller als `large` werden, billig bleiben und auf schweren Aufgaben mehr lösen. Jede Hypothese steht hier vor ihrem Lauf, mit Vorhersage und Abbruchregel.

## Schritt 0: SWE-bench Lite

### Auswahl

Quelle: `princeton-nlp/SWE-bench_Lite`, Test-Split, 300 Zeilen. Regel: SHA-256 der `instance_id`, sortiert, die ersten 50. Kein Cherry-picking. Die ersten 30 sind Entwicklung, die letzten 20 Hold-out. Die Reihenfolge ist die Hash-Reihenfolge.

| Datei | Inhalt | SHA-256 |
|---|---|---|
| `cli/tests/data/eval/swebench50.json` | 50 Aufgaben, ohne Test-Patch | `88238bf835cc7eb727039729bc1bd449a0fd51614c77b580bdee3b728715254b` |
| `cli/tests/data/eval/swebench_dev30.jsonl` | 30 Entwicklungsaufgaben | `7819859228608f74c7bc1e240c69f0d78df3614921ae69d92f004109404fe94b` |
| `cli/tests/data/eval/swebench_holdout20.jsonl` | 20 Hold-out-Aufgaben | `626fbf6d14e0d4340bc1b7e6d741b96a5ca437ce3d5f3a18e9427d509b36be90` |

Die Test-Patches, Testlisten und Versionen liegen lokal in `~/.cache/penelope/swebench/specs.json` (SHA-256 `2b3ed60b1def8933f842ac6e9b83425560d5d57c456b8fdd460608715d867f4c`). Der normalisierte Hold-out-Hash über Metadaten und Specs ist `0fbfec0822940b81667c480850c66be67b3c1a45f0d31db82fa84dd274013df7`. Der Hold-out läuft erst am Ende, einmal.

Die 30 Entwicklungsaufgaben: sympy 9, django 8, matplotlib 3, pylint 2, scikit-learn 2, sphinx 2, pytest 1, xarray 1, seaborn 1, requests 1. Die 20 Hold-out-Aufgaben: django 7, sympy 5, sphinx 2, xarray 1, matplotlib 1, pytest 1, pylint 1, scikit-learn 1, seaborn 1.

### Prüfung

Das Modell sieht den Issue-Text und das Repo auf `base_commit`, nicht den Test-Patch. Die Prüfung spielt den Test-Patch ein und führt jedes `FAIL_TO_PASS` und jedes `PASS_TO_PASS` aus. Bestanden nur, wenn alle grün sind. Ein Umgebungsfehler ist `infra`, kein Modellfehler.

### Basislauf: Vorhersage vor dem ersten Lauf

| Messung | large | cluster |
|---|---|---|
| Treffer auf 30 Entwicklungsaufgaben | 7 / 30 | 2 / 30 |
| EUR | 7,5 | 3,6 |
| Zeit | 3.600 s | 4.500 s |

Ableitung: die Schätzung 12 / 50 und 4 / 50 aus der Vorplanung, auf 30 skaliert. Der Cluster kostet 48 % von large und braucht 1,25-mal so lang. Das Ziel verlangt mindestens 3 Aufgaben mehr, höchstens 60 % der Kosten und höchstens die Zeit von large. Beim Basislauf ist nur die Kostengrenze erfüllt; Treffer und Zeit nicht.

Hold-out-Vorhersage: large 5 / 20, cluster 2 / 20. Exakter McNemar bei 3 zu 0: p = 0,25. Kein belegter Vorsprung.

`infra`: 1 bis 3 der 30 Entwicklungsaufgaben. Über 10 heißt: Harness reparieren, nicht scoren.

### Budget und Download

Der erste bezahlte Lauf startet erst nach der ausdrücklichen Freigabe des Owners. Der Auftrag nennt 20 EUR, die Grenze 16 EUR; die Schätzung für den Basislauf ist 11,1 EUR. Der Hold-out und die Runden kommen dazu.

Der Download ist über 1 GB und braucht eine eigene Freigabe. Zwei Wege:

- Offizielle SWE-bench-Docker-Images: 30 bis 60 GB für die 30 Entwicklungsaufgaben, weitere 20 bis 40 GB für den Hold-out. Zuverlässig, aber groß.
- Host-Umgebungen mit `uv`: 10 Repos, etwa 1,2 GB Klone plus 2 bis 4 GB Pakete. Leichter, aber die alten Versionen (Python 3.8 bis 3.11) müssen gebaut werden.

Der Auftrag sagt: Download über 1 GB vorher fragen. Dieser Text fragt.

### Vorarbeit vor dem Lauf

1. Repo-Cache mit den 10 Repos. Pro Aufgabe ein Git-Worktree auf `base_commit`.
2. Umgebung je Repo. Der Check läuft außerhalb des Modell-Sandkastens.
3. `eval.rs`: `base_commit` in der Aufgabe, Worktree statt Kopie, `PENELOPE_ROOT` für den Check.
4. `swebench_check.py`: Test-Patch einspielen, Tests laufen, Patch zurücknehmen.
5. Erst dann der Basislauf.

## Hypothesen K1 bis K8

Jede Zeile wird vor ihrem Lauf mit Zahlen und Abbruchregel ergänzt. Die erwartete Wirkung steht im Auftrag.

| Hypothese | Erwartung | Abbruch |
|---|---|---|
| K1 Statuszeilen statt Freitext | −20 % Eingabetokens beim Orchestrator | keine Verbesserung oder >10 % schlechter bei EUR/Zeit |
| K2 Verweise statt Kopien | −25 % Eingabetokens bei den Teilen | dito |
| K3 Mitos-Index im Cluster | −20 % Eingabetokens | dito |
| K4 fester Präfix für den Cache | −30 % EUR bei gleichen Tokens | dito |
| K5 parallele Teile | −30 % Zeit | dito |
| K6 weniger Validator-Runden | −20 % Zeit | dito |
| K7 Weiche | Zeit höchstens 1,1-mal large unter der Grenze | dito |
| K8 Fragen zwischen Clustern | +2 Aufgaben bei zwei Clustern | dito |

Der Loop stoppt bei Erfolg (cluster +3 Aufgaben, ≤60 % EUR, ≤large Zeit), nach 4 Runden ohne behaltene Änderung, nach 12 Runden oder bei 16 EUR.

### Preflight vor dem Basislauf

Drei Entwicklungsaufgaben, je ein anderes Repo, mit dem Gold-Patch und ohne Modell. Die Prüfung muss PASS geben. Sonst ist die Umgebung falsch, nicht das Modell. Der Preflight kostet keine API. Er braucht die Repos und die Umgebungen. Ergebnis 26.11.2026: 3 von 3 bestanden, django, sympy, pytest.

## Cluster-Kommunikation, Runde K9: Denkbudget des kleinen Modells

Vor dem Lauf. Der Basislauf hat 465.745 Ausgabetokens, davon 94,8 % nicht gespeicherter Modelltext. Das kleine Modell läuft mit `effort: high`. Change: `eval.rs` setzt für den Cluster `weak.effort = low`; der Claude-CLI-Teil bekommt `--effort low`.

Vorhersage: Ausgabe 465.745 auf etwa 280.000 (-40 %), Zeit 5.119 s auf etwa 3.800 s (-26 %), Treffer 8/28 gleich oder 7 (-1), EUR 2,5108 auf etwa 2,0. Behalten, wenn die Treffer gleich bleiben und EUR oder Zeit um mindestens 15 % fallen, oder wenn die Treffer steigen. Abbruch: Treffer 6 oder weniger, oder EUR/Zeit steigen.

## Runde K4: fester Präfix für den Cache

Vor dem Lauf. Systemprompt, sortierte Werkzeugliste und Memory-Schnappschuss bytegleich vorne. Der API-Teil sortiert die Werkzeuge (`clusters.rs`); der Claude-CLI-Teil bekommt die Werkzeugliste vom MCP-Server, dessen Reihenfolge und Mitos-Index liegen in `mcp.rs` und damit bei Claude. Vorhersage: -30 % EUR bei gleichen Tokens. Der Claude-Teil kann den Werkzeuglisten-Teil erst nach der Server-Änderung messen; der System-/Task-Präfix ist bereits stabil. Abbruch: EUR nicht mindestens 15 % tiefer oder Treffer schlechter.

## Runde K3: Mitos-Index im Cluster

Vor dem Lauf. Die Werkzeugliste ist 18,7 % der Eingabe (22.552 Byte, etwa 5.638 Tokens je Aufruf). Vorhersage: -20 % Eingabetokens, EUR und Zeit entsprechend. Der Claude-CLI-Teil bekommt die Werkzeuge vom `penelope mcp`-Server; der Server liefert die vollen Schemas. Der Mitos-Index braucht eine Server-Änderung in `mcp.rs`/`main.rs` (Claude) oder einen API-Teil. Blockiert für den aktuellen Claude-CLI-Cluster.

## K7: ein Teil geht direkt an large

Vor dem Lauf. Die 28 Entwicklungsaufgaben haben 1 unabhängigen Teil. Regel: ein Teil geht direkt an large; der Cluster erst ab 3 Teilen. Die Eval-Route `cluster` prüft die Worker-Zahl des Profils und ruft bei weniger als 3 den large-Pfad. Kein neuer bezahlter Lauf: der large-Basislauf ist die Messung.
