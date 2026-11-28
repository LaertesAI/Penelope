# Research Loop 3: Runden und Formate kleiner Modelle

Stand 28.11.2026. Arbeitsstand vor neuen Modellläufen. Lesestatus wie in `Forschungs-Loop.md`: (V) Volltext gelesen, (T) Abstract und relevante Abschnitte/Tabellen gelesen, (U) nur Treffer.

## Rang 5, Nachrechnen auf gespeicherten Zeilen

Daten: 340 Zeilen aus `~/.cache/penelope-calibrate-runs/exp/*/calibrate-hard_calib/results.jsonl`. Skript: `~/.penelope/tmp/deepseek/loop3_selection.py`. Ergebnis: `~/.penelope/tmp/deepseek/loop3_selection.json`.

Alte Regel (aa_band = 4): alle vier Modelle behalten `default`. Neue Regel (EUR je bestandener Aufgabe höchstens 80 % des Defaults, mindestens eine Aufgabe weniger): nur GLM wechselt auf `effort-low`. 1 von 4 Unterschieden. Die Vorhersage „mindestens 2" ist verworfen. Die Abbruchregel (0 oder 1 Unterschied) greift: die neue Regel wird nicht übernommen.

Zahlen je Variante (bestanden von 20, EUR, EUR je bestandener Aufgabe):

| Modell | Variante | Bestanden | EUR | EUR/Treffer |
|---|---|---|---|---|
| DeepSeek | default | 17 | 0,40182 | 0,02364 |
| DeepSeek | effort-low | 18 | 0,35175 | 0,01954 |
| GLM | default | 19 | 0,39924 | 0,02101 |
| GLM | effort-low | 19 | 0,31371 | 0,01651 |
| Kimi | default | 20 | 1,80976 | 0,09049 |
| Kimi | index | 16 | 0,99952 | 0,06247 |
| Haiku | default | 20 | 0,05216 | 0,00261 |
| Haiku | effort-low | 18 | 0,04073 | 0,00226 |

DeepSeek `effort-low` liegt mit 0,01954 knapp über der Schwelle 0,01891 und gewinnt deshalb nicht, obwohl er eine Aufgabe mehr besteht. GLM gewinnt, weil 0,01651 unter 0,01681 liegt. Eine zweite Kalibrierung je Modell zur Rauschmessung ist nicht gelaufen. Die vorhandenen A/A-Werte aus F97 (Kimi 4 von 28, Haiku 1 von 30) sind von einem anderen Aufgabensatz und ersetzen sie nicht.

## Rang 1, Power vor dem Fortschrittsdeckel

Exakter McNemar-Test, zweiseitig, alpha 0,05. Unter der Alternativhypothese b:c = 5:1 braucht es 17 diskordante Paare für 80 % Power. Bei 12 % diskordanten Paaren sind das etwa 142 Aufgaben. Der geplante Satz mit 50 Aufgaben liefert im Mittel 6 diskordante Paare und damit 33,5 % Power. Die geplante Behauptung b ≥ 5, c ≤ 1 ist mit 50 Aufgaben nicht entscheidbar.

Folge: Rang 1 wird in dieser Runde nicht gemessen. Ohne 140 Aufgaben pro Arm ist der Test unterpowert. Der 5-Stunden-Rahmen von OpenCode Go erlaubt nach F99 keine 280 Läufe in zwei Runden. Abbruch mit Zahl: 142 Aufgaben nötig, 50 geplant, Faktor 2,84 zu klein.

## Rang 6, Web-Research

### 1. W&D: Scaling Parallel Tool Calling, arXiv 2602.07359, 07.02.2026

(V, Abstract, Methoden 2.1, Ergebnisse 3.1 bis 3.2 und Abschnitt 5 gelesen.) GPT-5-Medium auf den ersten 100 BrowseComp-Aufgaben: drei Werkzeugaufrufe je Runde gaben 68 % gegen 66 % bei einem Aufruf, 23,8 gegen 45,7 Runden, 65,7 gegen 102,5 USD für 100 Aufgaben (minus 35,9 %) und 904,2 gegen 1522,6 s (minus 40,6 %). Der absteigende Scheduler (3, 2, 1 Aufrufe) erreichte 74 % in 23,5 Runden gegen 68 % bei konstant 3.

Hypothese für Penelope: Ein Variante `batch3`, die je Runde bis zu drei unabhängige Lesewerkzeuge bündelt, senkt die Werkzeugrunden je bestandener Aufgabe um mindestens 20 % gegenüber `default` bei höchstens 2 verlorenen Treffern auf 30 harten Aufgaben. Abbruch: unter 20 % Rundenersparnis oder mehr als 2 verlorene Treffer. Test: 30 Aufgaben, verschränkt, exakter McNemar, EUR und Runden je Treffer.

### 2. EET: Experience-Driven Early Termination, ACL Findings 2026, Guo u. a.

(T, Abstract und Tabellenwerte aus der Vorschau gelesen, Voll-PDF nicht abrufbar.) SWE-bench Verified, drei Agenten: Kosten 19 % bis 55 % niedriger, im Mittel 32 %, Verlust der Lösungsquote höchstens 0,2 Prozentpunkte. Frühes Stoppen tritt bei 11 % der Aufgaben ein. API-Aufrufe minus 21 %, Eingabe-Token minus 30 %, Ausgabe-Token minus 25 %.

Hypothese für Penelope: `stop-green` beendet einen Lauf nach dem ersten grünen Check nach einer Dateiänderung. Gegenüber 24 Runden sinkt EUR je bestandener Aufgabe um mindestens 20 %, c bleibt höchstens 1 auf 20 Aufgaben. Abbruch: c ≥ 2 oder weniger als 20 % EUR-Vorteil. Test: 20 Aufgaben, verschränkt, McNemar. Das ist Rang 2 des Auftrags; die Literaturenergie stützt den Hebel, nicht die Penelope-Zahl.

### 3. E-Bench, arXiv 2607.23722, 26.07.2026

(T, Abstract, Abschnitte 3 bis 4 und Anhang A.1 gelesen.) 323 Aufgaben, 11 Modelle. Code-Ausführung senkt MCP-Aufrufe je Aufgabe von 60,42 auf 15,86 (minus 73,8 %) und Runden von 14,87 auf 9,87 (minus 33,6 %), während Avg@3 steigt. Grok-4.5 mit 2,06 Aufrufen je Runde verbraucht 1,12 Mio. Token je Aufgabe; Modelle mit mindestens vier Aufrufen je Runde im Mittel 0,44 Mio.

Hypothese für Penelope: Ein `batch-code`-Werkzeug, das mehrere Lesewerkzeuge in einem Python-Block aufruft, senkt die Zahl der Werkzeugaufrufe je bestandener Aufgabe um mindestens 30 % bei mindestens gleicher Trefferzahl auf 20 Aufgaben. Abbruch: Treffer Verlust von mehr als 2 oder weniger als 30 % weniger Aufrufe. Test: 20 Aufgaben, verschränkt gegen `default`.

### 4. Don't Stop Early, ACL Industry 2026

(T, Abstract und Tabelle 3 aus der Quelle gelesen.) Plan-DAG mit abhängigkeitsgesteuerter Ausführung: 47 statt 222 Minuten, HAA 82,09 statt 76,40. Das Entfernen der expliziten Abschlusskriterien senkt HAA von 82,09 auf 73,67 und die öffentlichen Werkzeugaufrufe von 327 auf 224, also ein Earlier-Stopping mit schlechterem Ergebnis.

Hypothese für Penelope: `plan-first` mit einer listed Done-Bedingung pro Teilaufgabe senkt die Runden bis zum ersten grünen Check auf mehrdateiigen Aufgaben um mindestens 25 % bei höchstens 1 verlorenem Treffer. Abbruch: kein Rundengewinn oder mehr als 1 verlorener Treffer. Test: 20 Aufgaben aus den Familien rename und pipeline, verschränkt.

## Was die Quellen gemeinsam sagen und was nicht trägt

Alle vier Quellen messen weniger Runden oder weniger Aufrufe bei gleicher oder besserer Trefferquote. Keine Quelle testet ein kleines OpenCode-Go-Modell auf den harten Penelope-Aufgaben. Die Zahlen sind Autorenangaben, nicht auf Penelope nachgerechnet. Der Fortschrittsdeckel aus Rang 1 ist durch die Quellen motiviert, aber nicht belegt: EET stoppt auf Grundlage vergangener Erfahrung, nicht auf Grundlage einer Änderung im laufenden Arbeitsverzeichnis.
