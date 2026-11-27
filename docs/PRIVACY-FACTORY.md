# Privacy file factory (work plan P24, Paper 56)

`penelope privacy factory` writes synthetic German business documents with every personal item recorded as a gold
span. `penelope privacy bench` runs the privacy filter of the level "pii" over them and prints recall and precision
per kind.

## Hypotheses, written before the first measurement

Measured on 1,000 documents, seed 42 for development, seeds 7 and 2026 as hold-out.

| ID | Claim | Number that decides |
|----|-------|---------------------|
| H1 | The filter of `main` (level "pii") finds no names, addresses, phone numbers, tax ids or birth dates, because it has no rule for them. | recall 0 % for these five kinds |
| H2 | The filter of `main` finds most IBANs and e-mail addresses. IBANs followed by a full stop lose their last group, because the group rule refuses a group with punctuation. | IBAN recall 85 to 99 %, e-mail recall at least 99 %, precision 100 % for both |
| H3 | Rules with an IBAN check sum, name lists, name contexts (title, label, greeting, signature) and spreading a found name over the whole document reach the gate of P24. | name recall at least 98 %, IBAN recall at least 99.5 % on all three seeds |
| H4 | Names with a first name outside the detector list are found through context alone. | recall of unlisted names at least 95 % |
| H5 | Precision stays high. | precision at least 97 % over all kinds |
| H6 | The new rules mask little in source code, which holds almost no personal data. | at most 5 masks per 1,000 lines of `cli/src/*.rs` |
| H7 | The rules stay fast. | under 1 ms per document on average |
