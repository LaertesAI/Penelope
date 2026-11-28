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

Die Test-Patches und Testlisten liegen lokal in `~/.cache/penelope/swebench/specs.json` (SHA-256 `29a15b394a08983936b1d8b10b12071b935b07cfa0dfb6c505b3a299e89e991b`). Der normalisierte Hold-out-Hash über Metadaten und Specs ist `5fc3bc6df9d530fe42ca9e56b71b521b4a752df5ba854eeff05dee4eb5034a4c`. Der Hold-out läuft erst am Ende, einmal.

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

Risks: the harness is not a full SWE-bench agent. The resolved rate can be lower for both. If the environment fails
on more than 10 tasks, stop and fix the harness before scoring.

## Workflows C6: Review mit 3 und 1 Agenten

Vor dem Lauf. Derselbe Review-Workflow auf 10 bekannten Fehlern in einem Testrepo. Arm A: 3 Agenten, je ein Drittel der Dateien, jeder Befund wird von einem Prüfer bestätigt. Arm B: 1 Agent auf allen Dateien, gleiche Prüfung.

Vorhersage: Arm A findet 8 von 10, Arm B 6 von 10. Arm A kostet 1,4-mal Arm B und braucht 0,7-mal die Zeit. Abbruch: Arm A findet nicht mehr Fehler oder braucht über 1,2-mal die Zeit. Kein bezahlter API-Lauf, nur das Claude-Abo.

## Ergebnis C6

3 Agenten: 7/10, 0,0097 EUR, 67.855 Tokens. 1 Agent: 6/10, 0,0044 EUR, 20.760 Tokens. Der 3-Agenten-Arm findet einen Fehler mehr, nicht zwei. Er kostet 2,2-mal so viel, nicht 1,4-mal. Die Vorhersage war zu optimistisch. Behalten wird der 3-Agenten-Arm nur als Option, nicht als Standard; der 1-Agenten-Arm bleibt die Voreinstellung.

### Preflight vor dem Basislauf

Drei Entwicklungsaufgaben, je ein anderes Repo, mit dem Gold-Patch und ohne Modell. Die Prüfung muss PASS geben. Sonst ist die Umgebung falsch, nicht das Modell. Der Preflight kostet keine API. Er braucht die Repos und die Umgebungen. Ergebnis 26.11.2026: 3 von 3 bestanden, django, sympy, pytest.

## Runde 8 A2: Hold-out mit korrigierten Preisen

Vor dem Lauf. 20 eingefrorene Hold-out-Aufgaben, SHA-256 `626fbf6d14e0d4340bc1b7e6d741b96a5ca437ce3d5f3a18e9427d509b36be90`. Zwei Arme: Sonnet 5.5 und Haiku 5.5 mit `effort: low` (K9-Einstellung). Preise: Sonnet 2/10, Haiku 0,1/0,5 USD je Mio, 0,92 EUR je USD.

Vorhersage: large 5/20, K9 5/20, b = 0, c = 0, exakter McNemar p = 1,0. K9 kostet höchstens 20 % von large und braucht höchstens 1,0-mal die Zeit. Abbruch: K9 löst 2 oder mehr Aufgaben weniger als large, oder die Zeit liegt über 1,2-mal large.

Die 1 scikit-Aufgabe ist `infra` und wird ausgeschlossen.

## Ergebnis A3

Claude Code allein, Haiku 5.5 mit `effort: low`, 28 Entwicklungsaufgaben: 9/28. Penelope K9: 8/28. b = 1 (`sphinx-doc__sphinx-8595`), c = 0, exakter McNemar p = 1,0. Claude Code 0,1571 EUR und 1.546 s, K9 0,1446 EUR und 1.619 s. Claude Code ist 4,5 % schneller und 8,6 % teurer. Der eine Treffer ist nicht signifikant.

## Ergebnis A1

## Research Loop 3, Rang 5: Auswahlregel auf gespeicherten Zeilen

Stand 28.11.2026, vor dem Nachrechnen. Keine neuen Modellläufe in diesem Schritt. Daten: `~/.cache/penelope-calibrate-runs/exp`.

Neue Regel: Eine Variante gewinnt, wenn `eur / bestandene Aufgabe` höchstens 80 % des Defaults ist und die Variante höchstens eine Aufgabe weniger besteht als der Default (ein Aufgabe Spielraum). Bei 0 bestandenen Aufgaben im Default gibt es keine Auswahl. Bei mehreren Gewinnern: mehr bestandene Aufgaben, dann weniger EUR je Treffer, dann Default. Die alte Regel erlaubt einen Gewinn von `aa_band` Aufgaben oder Gleichstand bei höchstens 80 % der Token.

Vorhersage: Die neue Regel wählt bei mindestens 2 der 4 Modelle eine andere Variante als die alte. Abbruch: bei 0 oder 1 Unterschied wird die Regel nicht weiterverfolgt.

Rauschmessung: Eine zweite Kalibrierung je Modell ist nötig, bevor die neue Regel übernommen wird. Vorhersage: A/A-Unterschied mindestens 1 Aufgabe bei mindestens einem Modell. Abbruch: A/A überall 0, dann ist die Auswahl nicht von Rauschen zu trennen.

Rang 6, Web-Research: mindestens vier Primärquellen mit Lesestatus, jede mit einer prüfbaren Hypothese für Penelope und einem numerischen Ziel. Kein neues Modell, keine Bezahlung.

### Ergebnis der Regelprüfung, 28.11.2026

Mit der im Auftrag gemeinten Regel (höchstens ein Treffer weniger) wechselt 1 von 4 Modellen die Variante, nur GLM auf `effort-low`. Die Vorhersage mindestens 2 ist verworfen, die Abbruchregel greift. Bei der versehentlich wörtlich formulierten Regel (mindestens ein Treffer weniger) wären es 3 von 4; diese Lesart ist verworfen. Die Tabellenzahlen selbst sind aus 340 Rohzeilen reproduzierbar. Die 12 % Annahme der Powerrechnung ist nicht aus den Daten gestützt; ex-ante mit D ~ Bin(N, 0,12) sind es 19,1 % Power bei 50, 76,5 % bei 142 und 80,2 % bei 153 Aufgaben.
