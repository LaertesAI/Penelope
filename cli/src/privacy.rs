//! The privacy filter. It masks secrets in tool output before any model reads the text. Today it works with fixed
//! rules. The Strix model of VeeAI takes this place later.

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

#[cfg(test)]
mod tests {
    use super::mask;

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
