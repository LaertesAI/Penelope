# Research Loop 3: mathematische Vermutungen aus vorhandenen Daten

Stand 28.11.2026. Keine neuen Läufe. Alle Zahlen sind aus gespeicherten Zeilen gerechnet und als Vermutung markiert. Eine Vermutung ist belegt oder widerlegt erst durch einen Lauf, der jetzt nicht gestartet wird.

## V1, ein zurückgewonnener Treffer dreht das Ergebnis

Aus `stopgreen-r2b`: `default` 18 Treffer, 0,373117 EUR, 0,020729 EUR je Treffer. `stop-green` 17 Treffer, 0,355986 EUR, 0,020940 EUR je Treffer, also 1,02 % teurer. Bei gleichbleibenden Kosten und 18 statt 17 Treffern:

| Erfasste Treffer | EUR je Treffer | gegen Default |
|---|---|---|
| 17 | 0,020940 | +1,02 % |
| 18 | 0,019777 | −4,59 % |
| 19 | 0,018736 | −9,61 % |
| 20 | 0,017799 | −14,13 % |

Vermutung: H2 muss mindestens einen der drei Verluste zurückgewinnen, damit der Kostenhebel entsteht. Test mit vorhandenen Daten: nicht entscheidbar, weil die H2-Läufe abgebrochen wurden. Prüfweg später: 20verschränkte Paare, c höchstens 1.

## V2, der GLM-Wechsel liegt im A/A-Rauschen

GLM `effort-low` liegt bei 0,016511 EUR je Treffer, die 80-%-Schwelle bei 0,016810. Der Abstand ist 1,78 %. Die A/A-Diskordanz aus F97 beträgt 14 % bei Kimi und 3,3 % bei Haiku auf einem anderen Aufgabensatz. Vermutung: der einzige Regelwechsel ist ein Rauscheffekt, kein belastbarer Variantenwahlgewinn. Prüfweg: zweite Kalibrierung auf frischem Satz. Ohne Lauf bleibt V2 eine Vermutung.

## V3, der Fortschrittsdeckel braucht weniger als 153 Aufgaben, wenn die A/A-Rate gilt

Für b:c = 5:1 braucht der exakte McNemar-Test 17 diskordante Paare. Mit angenommener Diskordanz 12 % sind es 142 bis 153 Aufgaben. Die aus den Daten bekannten A/A-Raten liegen bei 14 % (Kimi) und 16 % (DeepSeek):

| Diskordanz | Aufgaben für 17 Paare |
|---|---|
| 12 % (Annahme) | 142 |
| 14 % (Kimi A/A) | 121 |
| 16 % (DeepSeek A/A) | 106 |

Vermutung: der echte Bedarf liegt zwischen 106 und 121 Aufgaben je Arm, nicht bei 153. Prüfweg: Diskordanz auf dem geplanten Aufgabensatz messen, dann N neu rechnen.

## V4, ein gezielter Rundenzuwachs kostet etwa 8,6 % statt 64,2 %

In `exp2` kostet `rounds-24` bei DeepSeek 0,030323 EUR je Aufgabe gegen 0,018468 im Default, plus 64,2 %. Die Erhöhung betrifft zwölf zusätzliche Runden. In `stopgreen-r2b` liefen 8 von 20 Default-Armen in den 12-Runden-Deckel. Bei linearer Kostenwirkung pro Runde kostet ein Zuwachs von vier Runden 4/12 · 64,2 % = 21,4 % für die betroffenen Läufe; gewichtet mit 8/20 ergibt sich 8,6 % für den ganzen Satz. Vermutung: ein Fortschrittsdeckel, der nur am Deckel zuschaltet, kostet rund 8,6 % EUR statt 64,2 %. Die Linearität ist eine Annahme, kein gemessener Wert.

## V5, bestätigte grüne Checks heben c von 3 auf höchstens 1

In den Historien von `hard-json-1` und `hard-inv-totals` war das grüne Signal ein `cat`/`echo` mit Exit 0 beziehungsweise ein fehlgeschlagener Edit; die Endantwort war DSML-Syntax. Zwei der drei c-Verluste sind damit erklärt. Vermutung: echte Testbefehle, zwei grüne Signale und ein Textfinale recovers genau diese zwei Fälle, c fällt von 3 auf höchstens 1, EUR je Treffer bleibt unter dem Default. Der Code dafür liegt in PR 68, die Läufe wurden auf Anweisung gestoppt. Ohne Lauf bleibt V5 eine Vermutung.

## Liste der Vermutungen zur weiteren Prüfung

| Nr | Vermutung | Zahl | Prüfweg ohne bezahlte Läufe |
|---|---|---|---|
| V1 | Ein Treffer mehr dreht den Kostenvergleich | −4,59 % statt +1,02 % | Kein; nur mit neuem Lauf |
| V2 | GLM-Wechsel ist Rauschen | 1,78 % Abstand | Zweite Kalibrierung, nicht gestartet |
| V3 | N liegt bei 106 bis 121 | 17 Paare | Diskordanz aus neuem Satz schätzen |
| V4 | Gezielter Deckel kostet 8,6 % | 64,2 % für alle | Linearität an gespeicherten Runden prüfen |
| V5 | H2 senkt c auf höchstens 1 | 2 von 3 erklärt | H2-Läufe, gestoppt |
