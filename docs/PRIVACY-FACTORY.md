# Privacy file factory (work plan P24, Paper 56)

The rules of the level "pii" now find 99.8 to 99.9 % of names and 100 % of IBANs on 1,000 factory documents per seed.
On documents that three other models wrote, a set measured once with frozen rules gives 92.9 % of names and 100 % of
IBANs. The gate of P24 (98 % of names and IBANs) holds on the factory and holds for IBANs everywhere. It does not yet
hold for names in text of other writers.

## Commands

```
penelope privacy factory --out DIR [--seed N] [--count N]   # documents plus gold.jsonl
penelope privacy bench [--seed N] [--count N]                # recall and precision per kind
penelope privacy bench --file cli/tests/data/privacy/round3-glm.txt --misses 20
penelope privacy bench --model                               # rules, then the model of roles.filter
penelope privacy scan FILE...                                # masks in any file, per 1,000 lines
```

## What was built

- `cli/src/privacy_bench.rs`: the factory. 1,000 documents per seed, a quarter each of invoices, contracts (employment,
  tenancy, service), e-mails and HR letters. Seven kinds of gold spans: name, iban, address, phone, email, tax_id,
  birth_date. SplitMix64 makes every seed give the same bytes on Linux, Windows and macOS. IBANs carry a valid ISO 13616
  check sum (DE, AT, CH, NL). Tax ids follow the digit rule of 2016 and the ISO 7064 MOD 11,10 check digit. 20 % of
  first names and about 55 % of surnames are outside the detector lists, so the bench measures context rules too.
- `cli/src/pii.rs`: the rules, behind the trait `Detector`. `privacy::mask_pii` and the `/redact` endpoint of
  `server.rs` both call `pii::spans` now. Before, the two had separate rules, and `server.rs` missed every IBAN written
  in groups of four.
- The bench scores masked text alone. `align` recovers which input bytes each `[masked:KIND]` replaced, with
  backtracking when the text after a mask also occurs inside it (`a@b.de.`). A model that returns masked text, such as
  Utis through `privacy::model_redact`, needs no span output and no change to the bench.
- `load_marked` reads documents with marks like `⟦name|Anna Berg⟧`. DeepSeek V4 Pro, GLM 5.3 and Kimi K3 wrote 168
  such documents through OpenCode Go (`cli/tests/data/privacy/`). `⟦iban⟧` and `⟦tax_id⟧` are filled with valid random
  numbers, because a writer rarely gets check digits right.

### Rules

| Kind | Rule |
|------|------|
| IBAN | one piece or groups of four; length by country and mod 97 check; a wrong check sum still counts after the word "IBAN" or "Konto" |
| Tax id | 11 digits, check digit; grouped, or with "Steuer", "IdNr" or "Identifikations" before |
| Phone | starts with 0, +49 or 0049; 9 to 15 digits; separators with spaces allowed (`0861 / 20 55 88`); contiguous numbers need "Tel", "Mobil" or "Fax" before; "Nr" or "Kunden" before blocks it |
| Birth date | a date with "geb.", "geboren", "Geburtsdatum" or a star right before |
| Address | 30 street endings (`-straße`, `-weg`, `-allee`), prefixes (`Am`, `An der`, `Zur`), any street right before a postcode, postcode with city at a line start or after a comma, `Postfach` |
| Name | titles (Herr, Frau, Dr., Familie), about 180 labels and role words (`Kontoinhaber:`, `Verwaltung:`, `Geprüft am 18.11.2026:`), greetings, signatures after a closing or above a role line, names above a street line, chat speakers and `@mentions`, table columns (`Nachname`, `PLZ`), list lines (`Wójcik, Marek (Produktion)`), first-name list (391 names), surname list (177 names), a role after the name (`, Geschäftsführer`, `(Vermieter)`) |
| Spread | a name word found once is masked everywhere in the same text; a capitalised word before a found name joins it; parts of `vorname.nachname@` seed the spread |

## Hypotheses, written before the first measurement

| ID | Claim | Number that decides | Result |
|----|-------|---------------------|--------|
| H1 | The filter of `main` finds no names, addresses, phone numbers, tax ids or birth dates. | recall 0 % | confirmed: 0 % on all three seeds |
| H2 | The filter of `main` finds most IBANs and all e-mail addresses. | IBAN 85 to 99 %, e-mail at least 99 % | IBAN refuted: 73.9 to 75.9 %; e-mail confirmed: 100 % |
| H3 | New rules reach the gate. | names at least 98 %, IBAN at least 99.5 % on three seeds | confirmed on the factory: names 99.81 to 99.94 %, IBAN 100 %; not met on text of other writers: names 92.9 % on the frozen set |
| H4 | Unlisted names are found through context. | at least 95 % | confirmed on the factory (99.24 to 99.78 %); refuted at first contact with other writers (51.7 % on the Kimi set of round 2) |
| H5 | Precision stays high. | at least 97 % over all kinds | confirmed: 100 % on the factory, 98.2 to 99.8 % on the six other sets |
| H6 | Few masks in source code. | at most 5 per 1,000 lines | confirmed: 0.43 (17 in 39,232 lines; 12 are real test e-mail addresses) |
| H7 | Fast. | under 1 ms per document | confirmed in release: 0.35 ms per document; 2 MB of text in 0.68 s, linear in length |

## Before and after on the factory

Seed 42 (development); seeds 7 and 2026 (hold-out) differ by at most 0.13 points. "Before" is commit 80d0cc0, the
filter of `main`.

| Kind | Gold | Recall before | Recall after | Precision after |
|------|------|---------------|--------------|-----------------|
| name | 5,376 | 0 % | 99.94 % | 100 % |
| iban | 969 | 75.85 % | 100 % | 100 % |
| address | 3,840 | 0 % | 100 % | 100 % |
| phone | 1,314 | 0 % | 100 % | 100 % |
| email | 1,545 | 100 % | 100 % | 100 % |
| tax_id | 333 | 0 % | 100 % | 100 % |
| birth_date | 399 | 0 % | 100 % | 100 % |
| all | 13,776 | 16.55 % | 99.98 % | 100 % |

The 234 IBANs that `main` missed were masked in part. An IBAN in groups followed by a full stop or a comma lost its
last group (`DE89 3704 0044 0532 0130 00.` kept `00`), and the hit counter still counted it as masked.

## Documents of other writers

Three rounds. The rules were changed only after a round was measured, so the first number of each set is a number on
unseen text.

| Set | Documents | Names at first contact | Names now | IBAN now | All kinds now | Precision now |
|-----|-----------|------------------------|-----------|----------|---------------|---------------|
| Round 1, DeepSeek | 43 | 53.7 % (aligner bug in 4 documents) | 98.8 % | 100 % | 99.4 % | 98.4 % |
| Round 1, GLM | 25 | 67.6 % (aligner bug in 1 document) | 97.1 % | 100 % | 98.7 % | 98.3 % |
| Round 2, Kimi | 25 | 60.7 % | 95.7 % | 100 % | 98.2 % | 98.2 % |
| Round 2, DeepSeek | 25 | 92.3 % | 95.6 % | 100 % | 98.2 % | 98.7 % |
| Round 3, Kimi | 25 | 80.4 % | 92.8 % | 100 % | 97.2 % | 99.8 % |
| Round 3, GLM | 25 | 92.9 % | 92.9 % (not tuned) | 100 % | 96.6 % | 98.5 % |

First-contact name recall rose from 53.7 to 67.6 % (round 1) over 60.7 to 92.3 % (round 2) to 80.4 to 92.9 % (round
3). A factory written by the same author as the rules overstated name recall by 7 to 39 points (rounds 2 and 3, where
the aligner was already correct).

## Why names still miss, and the next path

The last frozen set (round 3, GLM) missed 9 of 126 names:

- 4 in attendee lists under a label with a bracket (`Teilnehmende (alphabetisch):`) and their later surname-only mentions.
- 3 with initials (`O. Rehberg`, `Jan F. Rehberg` after `gez.`, `T. Fleischer-Zorn`).
- 2 in free text with an unlisted first name and no context (`Ihr Fahrer, Branimir Iliev, hat`).

The first two groups are rules: a label may carry a bracket, `gez.` starts a name with initials. The third group needs
a model. The seam is ready: `penelope privacy bench --model` runs the rules as the floor and the model of
`roles.filter` after them, scored from its masked text.

Next measurement: Utis (or any local filter model) on a fourth round of at least 50 unseen documents. It works if
names reach 98 % at precision of at least 97 %, with IBAN at 100 %. Until then the website may call the filter a
protection for IBANs, phone numbers and e-mail addresses (100 % on every set) and postal addresses (at least 98.9 %).
Tax ids reach 100 % on every set since column headers like `Steuer_ID` are read as `Steuer-ID` (before: 9 of 12 on
the Kimi set of round 3). For names it must say 93 % on text of other writers. Birth dates (93.8 % on one set: "am
02.02.1998 geboren") have too few items for a claim.

## Assumptions

- Every e-mail address and phone number counts as personal, also role addresses such as `info@` and switchboard
  numbers.
- Company addresses count as addresses. Company names, VAT ids, HRB numbers, invoice and customer numbers do not count.
- "Found" means every letter and digit of a gold item lies inside a mask. A mask counts as true when it overlaps any
  gold item, whatever its kind.
- The marks of the other models are the gold. They miss some items, for example group addresses like
  `alle-mitarbeiter@`, which costs precision.
- A title inside a name mark (`⟦name|Herr Krüger⟧`) is moved out before scoring.
- The name lists come from general knowledge of common names in Germany. No list was downloaded. The rare names of the
  factory stay outside the detector lists; a test enforces it.
- Model-written documents hold invented people. A name may match a real person by chance; no other data about anyone
  is in them.
