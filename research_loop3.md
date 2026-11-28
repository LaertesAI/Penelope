# Research Loop 3: Runden und Formate kleiner Modelle

Stand 28.11.2026. Arbeitsstand vor neuen Modellläufen. Lesestatus wie in `Forschungs-Loop.md`: (V) Volltext gelesen, (T) Abstract und relevante Abschnitte/Tabellen gelesen, (U) nur Treffer.

## Rang 5, Nachrechnen auf gespeicherten Zeilen

Daten: 340 Zeilen aus `~/.cache/penelope-calibrate-runs/exp/*/calibrate-hard_calib/results.jsonl`. Skript: `~/.penelope/tmp/deepseek/loop3_selection.py`. Ergebnis: `~/.penelope/tmp/deepseek/loop3_selection.json`.

Alte Regel (aa_band = 4, Messung): alle vier Modelle behalten `default`. Neue Regel (EUR je bestandener Aufgabe höchstens 80 % des Defaults, **höchstens eine Aufgabe weniger**): nur GLM wechselt auf `effort-low`. 1 von 4 Unterschieden. Der Auftrag meint mit „einer Aufgabe Spielraum" höchstens einen Treffer Verlust. Die Vorhersage „mindestens 2" ist verworfen, die Abbruchregel greift, die neue Regel wird nicht übernommen. Bei der versehentlich wörtlich formulierten Lesart „mindestens eine Aufgabe weniger" wären es 3 von 4; diese Lesart widerspricht dem Auftrag und ist verworfen. Das Skript prüft beide Prädikate mit Assert: 1 gegen 3.

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

DeepSeek `effort-low` liegt mit 0,01954 EUR je Treffer knapp über der Schwelle 0,01891 und gewinnt deshalb nicht, obwohl er eine Aufgabe mehr besteht (Messung aus den Rohzeilen). GLM gewinnt, weil 0,01651 unter 0,01681 liegt. Eine zweite Kalibrierung je Modell zur Rauschmessung ist nicht gelaufen. Die vorhandenen A/A-Werte aus F97 (Kimi 4 von 28, Haiku 1 von 30) sind von einem anderen Aufgabensatz und ersetzen sie nicht.

## Rang 1, Power vor dem Fortschrittsdeckel

Exakter McNemar-Test, zweiseitig, alpha 0,05 (abgeleitet). Unter b:c = 5:1 braucht es 17 diskordante Paare für 80 % Power. Die 12 % Diskordanz sind eine Annahme ohne direkte Datenstütze; mögliche Anhaltspunkte sind 14 % Kimi A/A, 16 % DeepSeek A/A und 18 % rounds-24 gegen 50. Ex-ante mit D ~ Bin(N, 0,12) ergibt die Simulation 19,1 % Power bei N = 50, 76,5 % bei N = 142 und 80,2 % bei N = 153. Bei N = 50 ist b = 5, c = 0 mit p = 0,0625 nie signifikant. Die bedingte Power von 33,5 % gilt nur für genau 6 diskordante Paare.

Folge: Rang 1 wird in dieser Runde nicht gemessen. Es braucht 153 Aufgaben je Arm, nicht 140. Der 5-Stunden-Rahmen von OpenCode Go erlaubt nach F99 keine 306 Läufe in zwei Runden. Abbruch mit Zahl: 153 nötig, 50 geplant, Faktor 3,06 zu klein.

## Rang 6, Web-Research

Alle Zahlen in den vier Quellenabschnitten sind Autorenangaben, nicht auf Penelope nachgerechnet.

### 1. W&D: Scaling Parallel Tool Calling, arXiv 2602.07359, 07.02.2026

(V, Abstract, Methoden 2.1, Ergebnisse 3.1 bis 3.2 und Abschnitt 5 gelesen.) GPT-5-Medium auf den ersten 100 BrowseComp-Aufgaben: drei Werkzeugaufrufe je Runde gaben 68 % gegen 66 % bei einem Aufruf, 23,8 gegen 45,7 Runden, 65,7 gegen 102,5 USD für 100 Aufgaben (minus 35,9 %) und 904,2 gegen 1522,6 s (minus 40,6 %). Der absteigende Scheduler (3, 2, 1 Aufrufe) erreichte 74 % in 23,5 Runden gegen 68 % bei konstant 3.

Hypothese für Penelope: Eine Variante `batch3`, die je Runde bis zu drei unabhängige Lesewerkzeuge bündelt, senkt die Werkzeugrunden je bestandener Aufgabe um mindestens 20 % gegenüber `default` bei höchstens 2 verlorenen Treffern auf 30 harten Aufgaben. Abbruch: unter 20 % Rundenersparnis oder mehr als 2 verlorene Treffer. Test: 30 Aufgaben, verschränkt, exakter McNemar, EUR und Runden je Treffer.

### 2. EET: Experience-Driven Early Termination, ACL Findings 2026, Guo u. a.

(T, Abstract und Tabellenwerte aus der Vorschau gelesen, Voll-PDF nicht abrufbar.) SWE-bench Verified, drei Agenten: Kosten 19 % bis 55 % niedriger, im Mittel 32 %, Verlust der Lösungsquote höchstens 0,2 Prozentpunkte. Frühes Stoppen tritt bei 11 % der Aufgaben ein. API-Aufrufe minus 21 %, Eingabe-Token minus 30 %, Ausgabe-Token minus 25 %.

Hypothese für Penelope (geprüft in Runde 2): `stop-green` beendet einen Lauf nach dem ersten grünen Check nach einer Dateiänderung. Vergleich gegen `default` mit 12 Runden, nicht gegen `rounds-24`, weil dessen EUR Aufwand bereits um 68,6 % höher ist. Ziel: c = 0 und EUR je bestandener Aufgabe mindestens 20 % niedriger, gepaarter Sign-Flip-CI der EUR-Differenz mit Obergrenze unter −20 %. Abbruch: c ≥ 1 oder CI-Obergrenze bei mindestens −20 %. Test: 20 verschränkte Paare, exakter McNemar.

### 3. E-Bench, arXiv 2607.23722, 26.07.2026

(T, Abstract, Abschnitte 3 bis 4 und Anhang A.1 gelesen.) 323 Aufgaben, 11 Modelle. Code-Ausführung senkt MCP-Aufrufe je Aufgabe von 60,42 auf 15,86 (minus 73,8 %) und Runden von 14,87 auf 9,87 (minus 33,6 %), während Avg@3 steigt. Grok-4.5 mit 2,06 Aufrufen je Runde verbraucht 1,12 Mio. Token je Aufgabe; Modelle mit mindestens vier Aufrufen je Runde im Mittel 0,44 Mio.

Hypothese für Penelope: Ein `batch-code`-Werkzeug, das mehrere Lesewerkzeuge in einem Python-Block aufruft, senkt die Zahl der Werkzeugaufrufe je bestandener Aufgabe um mindestens 30 %. Ziel c = 0, Bootstrap-95 %-CI der gepaarten Differenz mit Obergrenze unter −20 %. Abbruch: c ≥ 1 oder CI-Obergrenze bei mindestens −20 %. Test: 20 verschränkte Paare, exakter McNemar plus Bootstrap.

### 4. Don't Stop Early, ACL Industry 2026

(T, Abstract und Tabelle 3 aus der Quelle gelesen.) Plan-DAG mit abhängigkeitsgesteuerter Ausführung: 47 statt 222 Minuten, HAA 82,09 statt 76,40. Das Entfernen der expliziten Abschlusskriterien senkt HAA von 82,09 auf 73,67 und die öffentlichen Werkzeugaufrufe von 327 auf 224, also ein Earlier-Stopping mit schlechterem Ergebnis.

Hypothese für Penelope: `plan-first` mit einer listed Done-Bedingung pro Teilaufgabe senkt die Runden bis zum ersten grünen Check auf mehrdateiigen Aufgaben um mindestens 25 % bei höchstens 1 verlorenem Treffer. Abbruch: kein Rundengewinn oder mehr als 1 verlorener Treffer. Test: 20 Aufgaben aus den Familien rename und pipeline, verschränkt.

## Was die Quellen gemeinsam sagen und was nicht trägt

Drei der vier Quellen messen weniger Runden oder weniger Aufrufe bei gleicher oder besserer Trefferquote. Die vierte, Don't Stop Early, zeigt das Gegenteil: ohne Abschlusskriterien sinken die Werkzeugaufrufe von 327 auf 224, der HAA aber von 82,09 auf 73,67. Keine Quelle testet ein kleines OpenCode-Go-Modell auf den harten Penelope-Aufgaben. Die Zahlen sind Autorenangaben, nicht auf Penelope nachgerechnet. Der Fortschrittsdeckel aus Rang 1 ist durch die Quellen motiviert, aber nicht belegt: EET stoppt auf Grundlage vergangener Erfahrung, nicht auf Grundlage einer Änderung im laufenden Arbeitsverzeichnis.
