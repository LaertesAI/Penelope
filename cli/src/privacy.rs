//! The privacy filter. It masks secrets in requests and tool output before any model reads the text. Today it works
//! with fixed rules. An Utis model takes this place later. The level "pii" also masks e-mail addresses and IBANs.

const KEY_NAMES: &[&str] = &["password", "passwd", "secret", "token", "api_key", "apikey", "api-key", "access_key", "private_key", "authorization"];
/// Prefix of a known key format and the shortest length that counts.
const PREFIXES: &[(&str, usize)] = &[("sk-", 20), ("ghp_", 20), ("gho_", 20), ("ghs_", 20), ("github_pat_", 30), ("glpat-", 20), ("xoxb-", 20),
                                     ("xoxp-", 20), ("hf_", 20), ("AIza", 30), ("AKIA", 20)];

#[derive(PartialEq)]
enum Wait {
    Nothing,
    /// The last word was a key name such as `password`.
    Separator,
    /// A key name and `=` or `:` came before, the next word is the value.
    Value,
}

fn is_delim(c: char) -> bool {
    c.is_whitespace() || matches!(c, '"' | '\'' | ',' | ';' | '(' | ')' | '<' | '>' | '`' | '[' | ']' | '{' | '}')
}

fn is_key_name(word: &str) -> bool {
    let w = word.trim_start_matches(['-', '$']).to_lowercase();
    !w.is_empty() && w.len() <= 40 && KEY_NAMES.iter().any(|k| w.contains(k))
}

/// A value counts as a secret if it has 6 characters or more and is no plain number.
fn is_value(word: &str) -> bool {
    word.chars().count() >= 6 && !word.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | '%'))
}

fn known_key(word: &str) -> bool {
    if PREFIXES.iter().any(|(p, min)| word.starts_with(p) && word.len() >= *min) {
        return true;
    }
    word.starts_with("eyJ") && word.len() >= 30 && word.matches('.').count() == 2      // a signed web token
}

fn word(w: &str, wait: &mut Wait, hits: &mut usize) -> String {
    if known_key(w) {
        *wait = Wait::Nothing;
        *hits += 1;
        return "[masked:key]".to_string();
    }
    if *wait == Wait::Value {
        if w == "Bearer" || w == "Basic" || w == "=" || w == ":" {
            return w.to_string();
        }
        *wait = Wait::Nothing;
        if is_value(w) {
            *hits += 1;
            return "[masked:secret]".to_string();
        }
        return w.to_string();
    }
    if *wait == Wait::Separator {
        *wait = if w == "=" || w == ":" { Wait::Value } else { Wait::Nothing };
        if *wait == Wait::Value {
            return w.to_string();
        }
    }
    if let Some(at) = w.find(['=', ':']) {
        let (key, value) = (&w[..at], &w[at + 1..]);
        if is_key_name(key) && !value.starts_with("//") {
            if value.is_empty() {
                *wait = Wait::Value;
            } else if is_value(value) {
                *hits += 1;
                return format!("{}{}[masked:secret]", key, &w[at..at + 1]);
            }
        }
        return w.to_string();
    }
    if is_key_name(w) {
        *wait = Wait::Separator;
    }
    w.to_string()
}

/// The text with every secret replaced by `[masked:...]`, and the number of replacements.
pub fn mask(text: &str) -> (String, usize) {
    let (mut out, mut hits, mut in_key) = (String::with_capacity(text.len()), 0usize, false);
    for line in text.split_inclusive('\n') {
        if in_key {
            in_key = !line.contains("-----END");
            continue;
        }
        if line.contains("-----BEGIN") && line.contains("PRIVATE KEY") {
            in_key = !line.contains("-----END");
            hits += 1;
            out.push_str("[masked:private key]\n");
            continue;
        }
        let (mut wait, mut current) = (Wait::Nothing, String::new());
        for c in line.chars() {
            if is_delim(c) {
                if !current.is_empty() {
                    out.push_str(&word(&current, &mut wait, &mut hits));
                    current.clear();
                }
                out.push(c);
            } else {
                current.push(c);
            }
        }
        if !current.is_empty() {
            out.push_str(&word(&current, &mut wait, &mut hits));
        }
    }
    (out, hits)
}

fn is_email(w: &str) -> bool {
    let Some((local, domain)) = w.split_once('@') else { return false };
    !local.is_empty() && !domain.contains('@') && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
        && local.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'))
        && domain.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
}

/// Country code, two check digits, then 11 to 30 letters or digits.
fn is_iban(w: &str) -> bool {
    let b = w.as_bytes();
    (15..=34).contains(&b.len()) && b[0].is_ascii_uppercase() && b[1].is_ascii_uppercase() && b[2].is_ascii_digit() && b[3].is_ascii_digit()
        && b[4..].iter().all(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
}

/// The second pass of the level "pii": e-mail addresses and IBANs, written in one piece or in groups of four.
pub fn mask_pii(text: &str) -> (String, usize) {
    let (mut out, mut hits) = (String::with_capacity(text.len()), 0usize);
    for line in text.split_inclusive('\n') {
        let (body, end) = match line.strip_suffix('\n') {
            Some(b) => (b, "\n"),
            None => (line, ""),
        };
        let words: Vec<&str> = body.split(' ').collect();
        let mut i = 0;
        let mut first = true;
        while i < words.len() {
            if !first {
                out.push(' ');
            }
            first = false;
            let w = words[i];
            let core = w.trim_matches(|c: char| is_delim(c) || matches!(c, '.' | ':' | '!' | '?'));
            if is_email(core) {
                out.push_str(&w.replace(core, "[masked:email]"));
                hits += 1;
            } else if is_iban(core) {
                out.push_str(&w.replace(core, "[masked:iban]"));
                hits += 1;
            } else {
                // an IBAN in groups: DE89 3704 0044 0532 0130 00
                let mut joined = String::new();
                let mut j = i;
                while j < words.len() && joined.len() < 34 && !words[j].is_empty() && words[j].len() <= 4
                    && words[j].chars().all(|c| c.is_ascii_digit() || c.is_ascii_uppercase()) {
                    joined.push_str(words[j]);
                    j += 1;
                }
                if j - i >= 4 && is_iban(&joined) {
                    out.push_str("[masked:iban]");
                    hits += 1;
                    i = j;
                    continue;
                }
                out.push_str(w);
            }
            i += 1;
        }
        out.push_str(end);
    }
    (out, hits)
}

#[cfg(test)]
mod tests {
    use super::{mask, mask_pii};

    #[test]
    fn masks_mail_addresses_and_ibans() {
        let (t, n) = mask_pii("Write to (anna.berg@example.org). IBAN DE89 3704 0044 0532 0130 00 or DE89370400440532013000\nplain line 1234 5678\n");
        assert_eq!(n, 3, "{}", t);
        assert_eq!(t, "Write to ([masked:email]). IBAN [masked:iban] or [masked:iban]\nplain line 1234 5678\n");
        for plain in ["user@host", "git@github.com:owner/repo.git", "x = a @ b", "AB12 is short"] {
            assert_eq!(mask_pii(plain), (plain.to_string(), 0));
        }
    }

    #[test]
    fn masks_keys_and_values() {
        let (t, n) = mask("export OPENAI_API_KEY=sk-abcdefghijklmnopqrstuvwx\nDB_PASSWORD=hunter2hunter\n");
        assert_eq!(n, 2, "{}", t);
        assert!(!t.contains("sk-abc") && !t.contains("hunter2"), "{}", t);
        assert!(t.contains("DB_PASSWORD=[masked:secret]"), "{}", t);
        let (t, n) = mask("{\"api_key\": \"q7Lw2ZxP91aa\", \"name\": \"demo\"}");
        assert_eq!(n, 1, "{}", t);
        assert!(t.contains("\"name\": \"demo\"") && !t.contains("q7Lw2"), "{}", t);
        let (t, n) = mask("Authorization: Bearer abc.def.ghi-123456");
        assert_eq!((n, t.as_str()), (1, "Authorization: Bearer [masked:secret]"));
    }

    #[test]
    fn masks_private_key_blocks() {
        let (t, n) = mask("before\n-----BEGIN OPENSSH PRIVATE KEY-----\nAAAA\nBBBB\n-----END OPENSSH PRIVATE KEY-----\nafter\n");
        assert_eq!((n, t.as_str()), (1, "before\n[masked:private key]\nafter\n"));
    }

    #[test]
    fn leaves_plain_text_alone() {
        for plain in ["tokens so far: 9601 in, 398 out", "max_tokens=4096", "the token expired yesterday", "see https://example.org/a?b=c",
                      "fn password_hash(input: &str) -> String", "error: 3 tests failed in src/main.rs:42"] {
            let (t, n) = mask(plain);
            assert_eq!((n, t.as_str()), (0, plain));
        }
    }
}
