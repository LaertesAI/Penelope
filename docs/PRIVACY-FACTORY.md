# Privacy file factory (work plan P24, Paper 56)

The rules of the level "pii" find 99.8 to 99.9 % of names and 100 % of IBANs on 1,000 factory documents per seed. On
documents that three other models wrote, the last set measured with frozen rules gave 92.9 % of names (117 of 126) and
100 % of IBANs. The rules are tuned on all six sets now: pooled names 752 of 773 (97.3 %), lowest set 92.75 % (round 3,
Kimi). The gate of P24 (98 % of names and IBANs) holds on the factory and for IBANs everywhere. For names on unseen text
of other writers it is not shown: no unseen set is left, and round 4 decides it.

The numbers hold through the tools too. The read tool puts a line number before each line, grep puts `path:line: `,
and Windows files end lines with `\r\n`. The rules now run on the text without them (`pii::spans`), and
`privacy bench --view read|grep|crlf` gives the same counts as `--view raw` on every set.

## Commands

```
penelope privacy factory --out DIR [--seed N] [--count N]   # documents plus gold.jsonl
penelope privacy bench [--seed N] [--count N]                # recall and precision per kind
penelope privacy bench --file cli/tests/data/privacy/round3-glm.txt --misses 20
penelope privacy bench --file FILE --view read              # as the read tool shows it; also grep, crlf
penelope privacy bench --model                               # rules, then the model of roles.filter (local only)
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
- `pii::spans` strips a read prefix (`    5  `, the number never starts with 0) or a grep prefix (`path:5: `) when
  every line carries one, and the `\r` of `\r\n`. It runs the rules on that plain text and maps each span back to the
  input bytes. Before, the line-anchored rules (signature, lone title, label on the line before, list line, chat
  speaker, street before a postcode) never fired on read output.
- `files::write` and `files::edit` refuse new text that holds more `[masked:` placeholders than the file. Read and
  grep show names masked, so a model that copies a line it read would write `[masked:name]` over the real name. With
  the privacy level "off" nothing is masked and nothing is refused.
- `privacy bench --model` refuses a `roles.filter` model that does not run on localhost, 127.0.0.1 or [::1], because
  the bench sends every document to it. `--cloud` overrides.
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
| Name | titles (Herr, Frau, Dr., Familie), about 180 labels and role words (`Kontoinhaber:`, `Name des Empfängers:`, `Geprüft am 18.11.2026:`), a role with a comma before a name that closes with a comma (`Ihr Fahrer, Branimir Iliev, hat`), greetings, signatures after a closing or above a role line, names above a street line, chat speakers and `@mentions`, table columns (`Nachname`, `PLZ`; a plain `Name` as the only personal column masks only cells that look like a person's name), list lines (`Wójcik, Marek (Produktion)`, also when a rule masked one of the two words first), initials (`gez. O. Rehberg`, `Jan F. Rehberg`, not the `A.` of `i. A.`), first-name list (391 names), surname list (177 names), a role after the name (`, Geschäftsführer`, `(Vermieter)`) |
| Spread | a name word found once is masked everywhere in the same text; a surname under 3 letters spreads only after a list line `Vo, Duc`; a capitalised word before a found name joins it; parts of `vorname.nachname@` seed the spread |

## Hypotheses, written before the first measurement

| ID | Claim | Number that decides | Result |
|----|-------|---------------------|--------|
| H1 | The filter of `main` finds no names, addresses, phone numbers, tax ids or birth dates. | recall 0 % | confirmed: 0 % on all three seeds |
| H2 | The filter of `main` finds most IBANs and all e-mail addresses. | IBAN 85 to 99 %, e-mail at least 99 % | IBAN refuted: 73.9 to 75.9 %; e-mail confirmed: 100 % |
| H3 | New rules reach the gate. | names at least 98 %, IBAN at least 99.5 % on three seeds | confirmed on the factory: names 99.81 to 99.94 %, IBAN 100 %; not met on text of other writers: names 92.9 % on the frozen set |
| H4 | Unlisted names are found through context. | at least 95 % | confirmed on the factory (99.24 to 99.78 %); refuted at first contact with other writers (51.7 % on the Kimi set of round 2) |
| H5 | Precision stays high. | at least 97 % over all kinds | confirmed under the overlap definition: 100 % on the factory, 98.2 to 99.8 % on the six other sets. Byte precision: 100 % on the factory, 96.7 to 99.6 % on the six sets (round 1, DeepSeek below 97 %) |
| H6 | Few masks in source code. | at most 5 per 1,000 lines | confirmed for code: 0.43 (17 in 39,232 lines), now 0.89 on `cli/src` after the merge of `main` (43 in 48,438 lines, without `pii.rs` and `privacy_bench.rs`). Refuted for docs until this fix: 6.99 (39 in 5,577 lines of `docs/*.md`, 36 of them `Name` cells of a provider table); now 0.72 (4 masks) |
| H7 | Fast. | under 1 ms per document | confirmed per document: 0.38 ms in release. Refuted for long text before this fix: name-dense text cost 0.18, 0.34, 0.83, 2.31 and 16.2 s for 0.25 to 4 MB (quadratic spread). Now 0.17, 0.28, 0.50, 0.95 and 1.74 s, each doubling 1.6 to 1.9 times |

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

The 234 IBANs that `main` missed on seed 42 (24.1 to 26.1 % over the three seeds) were masked in part. An IBAN in
groups followed by a full stop or a comma kept its last group open, 1 to 4 characters: `00` for DE and NL, 4 digits for
AT, 1 character for CH. The first groups still passed the length test, so the hit counter counted the IBAN as masked.

## Documents of other writers

Three rounds. The rules were changed only after a round was measured, so the first number of each set is a number on
unseen text.

| Set | Documents | Names at first contact | Names now | IBAN now | All kinds now | Precision now |
|-----|-----------|------------------------|-----------|----------|---------------|---------------|
| Set | Documents | Names at first contact | Names now | IBAN now | All kinds now | Precision now | Byte precision now |
|-----|-----------|------------------------|-----------|----------|---------------|---------------|--------------------|
| Round 1, DeepSeek | 43 | 53.7 % (aligner bug in 4 documents) | 98.8 % | 100 % | 99.4 % | 98.4 % | 96.7 % |
| Round 1, GLM | 25 | 67.6 % (aligner bug in 1 document) | 97.1 % | 100 % | 98.7 % | 98.3 % | 98.9 % |
| Round 2, Kimi | 25 | 60.7 % | 96.6 % | 100 % | 98.6 % | 98.2 % | 97.7 % |
| Round 2, DeepSeek | 25 | 92.3 % | 98.9 % | 100 % | 99.6 % | 98.7 % | 98.4 % |
| Round 3, Kimi | 25 | 80.4 % | 92.8 % | 100 % | 97.2 % | 99.8 % | 99.6 % |
| Round 3, GLM | 25 | 92.9 % (frozen) | 100 % (tuned in review) | 100 % | 100 % | 98.8 % | 98.9 % |
| Pooled | 168 | 73.0 % (564/773) | 97.3 % (752/773) | 100 % (82/82) | 98.8 % | 98.7 % | 98.4 % |

Precision counts a mask as true when it overlaps any gold item. Byte precision counts only the masked bytes that lie
inside a gold item of the same kind, so a mask that runs past a name into a role word loses those bytes.

First-contact name recall rose from 53.7 to 67.6 % (round 1) over 60.7 to 92.3 % (round 2) to 80.4 to 92.9 % (round
3). Against these numbers the factory (99.94 % on seed 42) overstated name recall by 7.1 points (round 3, GLM) to 39.2
points (round 2, Kimi) in rounds 2 and 3, and 19.5 points on round 3, Kimi. With the aligner bug of round 1 the gap
reached 46.2 points. The smaller gap of round 3 may come from rules tuned on earlier rounds; convergence is a
hypothesis until round 4.

## Why names missed, and the next path

The frozen set (round 3, GLM) missed 9 of 126 names. All 9 were rule gaps, so they do not show that names need a
model:

- 4 in an attendee list: the token rule masked the listed first name alone (`Amrani, Omar` gave only `Omar`), and the
  list rule then refused the line. `Amrani` and `Kastenmacher` stayed open in the list and in later mentions.
- 1 surname of 2 letters: `Vo, Duc` was masked in the list, `Vo berichtet` was not, because the spread skipped words
  under 3 letters.
- 1 role before a comma: `Ihr Fahrer, Branimir Iliev, hat`.
- 3 initials: `O. Rehberg` (the surname was masked alone first), `Jan F. Rehberg` (the group stopped at `F.`),
  `Name des Empfängers: T. Fleischer-Zorn` (the label had words between `Name` and the colon).

The fixes give 126 of 126 on that set, precision 98.8 %, factory unchanged (99.94 %, 99.85 %, 99.81 %; precision
100 %). The set is tuned now, so 100 % is no measurement on unseen text.

The read path lost names before this fix. Review measured pooled names 95.6 % on raw text and 87.3 % with the read
prefix (round 2, Kimi 95.73 to 75.21 %; round 3, Kimi addresses 99.07 to 88.89 %), while the factory dropped only from
99.91 to 99.72 %. That number comes from review alone and was not recounted. After the fix the four views give equal
counts on all six sets, and a test fails when a view falls more than 1 point below raw.

Next measurement: round 4, written adversarially through OpenCode Go with the rule table of this file as the brief,
rules frozen before the first count. Size: about 200 documents (about 1,000 names at 5.04 per document). 50 documents
can detect a rise from 92.9 to 98 % (McNemar, 99 % power), but cannot certify 98 %: with true recall exactly 98 %, a
point estimate of at least 98 % comes out 61 % of the time on 252 names. A claim of more than 98 % at a true 99 % needs
about 969 names. The gate passes when the one-sided 95 % Clopper-Pearson bound and a document-clustered bootstrap bound
are both at least 98 % on at least 969 names. Then `privacy bench --model` runs Utis (or any filter model on this
machine) after the rules on the same set.

Two more paths, not built yet:

- A session name memory in RAM. `guard_count` runs once per 300-line read chunk and once per grep result, so a surname
  found through `Frau Berg` in chunk 1 is open in chunk 2. Test: split each of the 168 documents in two, mask the halves
  in two calls, measure the drop first; with the memory, recall within 0.5 points of whole documents.
- One pass of regex checks per token. A crafted 1 MB text costs 0.27 s (e-mail locals) to 1.16 s (initials) in release
  at load average 17 to 21, linear but above the 0.5 s goal for 4 of 5 crafted inputs. Before this fix they cost 3.9
  to 32 s on the same machine.

The website may call the filter a protection for IBANs, phone numbers and e-mail addresses: 100 % on every set
(pooled 82 of 82, 180 of 180, 154 of 154) through all four views. Postal addresses: 557 of 558 pooled, lowest set
99.07 %, also through all views. Tax ids reach 100 % on every set since column headers like `Steuer_ID` are read as
`Steuer-ID` (before: 9 of 12 on the Kimi set of round 3). Birth dates reach 94 of 94 since a date followed by
`geboren` counts. For names it must say 93 % on text of other writers, the last frozen number.

## Assumptions

- Every e-mail address and phone number counts as personal, also role addresses such as `info@` and switchboard
  numbers.
- Company addresses count as addresses. Company names, VAT ids, HRB numbers, invoice and customer numbers do not count.
- "Found" means every letter and digit of a gold item lies inside a mask. A mask counts as true when it overlaps any
  gold item, whatever its kind. Byte precision is the stricter second number.
- The floor test pools the six sets: names at least 96.5 %, precision at least 98 %, IBAN 100 %, and each set at least
  92 % of names. It is no per-set claim of 96.5 %.
- The marks of the other models are the gold. They miss some items, for example group addresses like
  `alle-mitarbeiter@`, which costs precision.
- A title inside a name mark (`⟦name|Herr Krüger⟧`) is moved out before scoring.
- The name lists come from general knowledge of common names in Germany. No list was downloaded. The rare names of the
  factory stay outside the detector lists; a test enforces it.
- Model-written documents hold invented people. A name may match a real person by chance; no other data about anyone
  is in them.
