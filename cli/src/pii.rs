//! Personal data in German business text. `spans` finds names, IBANs, postal addresses, phone numbers, e-mail
//! addresses, tax ids and birth dates with fixed rules, and `mask` replaces each span by `[masked:KIND]`.
//!
//! The trait `Detector` is the seam for a model. The rules implement it today. An Utis model implements it later by
//! returning masked text, and `privacy_bench` scores any implementation from that masked text alone, so the model
//! needs no span output and no change to the bench.

use regex::Regex;
use std::collections::HashSet;
use std::sync::OnceLock;

/// One personal item: byte range in the text and its kind (`name`, `iban`, `address`, `phone`, `email`, `tax_id`,
/// `birth_date`).
#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub kind: &'static str,
}

/// Anything that masks personal data. `mask` returns the text with each item replaced by `[masked:KIND]` and every
/// other character unchanged.
pub trait Detector {
    fn name(&self) -> String;
    fn mask(&self, text: &str) -> String;
}

/// The fixed rules of this file.
pub struct Rules;

impl Detector for Rules {
    fn name(&self) -> String {
        "rules".to_string()
    }
    fn mask(&self, text: &str) -> String {
        mask(text).0
    }
}

/// The text with every span replaced by `[masked:KIND]`, and the number of spans.
pub fn mask(text: &str) -> (String, usize) {
    let found = spans(text);
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for s in &found {
        out.push_str(&text[last..s.start]);
        out.push_str("[masked:");
        out.push_str(s.kind);
        out.push(']');
        last = s.end;
    }
    out.push_str(&text[last..]);
    (out, found.len())
}

/// All personal items in the text, sorted, without overlaps. A kind found earlier in this list wins an overlap.
pub fn spans(text: &str) -> Vec<Span> {
    let mut taken = Taken::default();
    emails(text, &mut taken);
    ibans(text, &mut taken);
    phones(text, &mut taken);
    tax_ids(text, &mut taken);
    birth_dates(text, &mut taken);
    addresses(text, &mut taken);
    names(text, &mut taken);
    let mut v = taken.0;
    v.sort_by_key(|s| s.start);
    v
}

/// Accepted spans. A new span that overlaps an accepted one is dropped.
#[derive(Default)]
struct Taken(Vec<Span>);

impl Taken {
    fn free(&self, start: usize, end: usize) -> bool {
        !self.0.iter().any(|s| start < s.end && s.start < end)
    }
    fn add(&mut self, start: usize, end: usize, kind: &'static str) -> bool {
        if start >= end || !self.free(start, end) {
            return false;
        }
        self.0.push(Span { start, end, kind });
        true
    }
}

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).unwrap())
}

fn is_delim(c: char) -> bool {
    c.is_whitespace() || matches!(c, '"' | '\'' | ',' | ';' | '(' | ')' | '<' | '>' | '`' | '[' | ']' | '{' | '}' | '„' | '“' | '”' | '‚' | '‘' | '’')
}

/// Whitespace-separated words with the outer punctuation cut off: (start, end) of the core.
fn cores(text: &str) -> Vec<(usize, usize)> {
    static WORD: OnceLock<Regex> = OnceLock::new();
    let mut out = Vec::new();
    for m in re(&WORD, r"\S+").find_iter(text) {
        let w = m.as_str();
        let lead = w.len() - w.trim_start_matches(is_delim).len();
        let core = w.trim_start_matches(is_delim).trim_end_matches(|c: char| is_delim(c) || matches!(c, '.' | ':' | '!' | '?'));
        if !core.is_empty() {
            out.push((m.start() + lead, m.start() + lead + core.len()));
        }
    }
    out
}

// ---------------------------------------------------------------- e-mail

fn is_email(w: &str) -> bool {
    let Some((local, domain)) = w.split_once('@') else { return false };
    !local.is_empty() && !domain.contains('@') && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
        && local.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'))
        && domain.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
}

fn emails(text: &str, taken: &mut Taken) {
    for (s, e) in cores(text) {
        if is_email(&text[s..e]) {
            taken.add(s, e, "email");
        }
    }
}

// ---------------------------------------------------------------- IBAN

/// Length of an IBAN by country, for the countries a German business text names most.
fn iban_length(country: &str) -> Option<usize> {
    Some(match country {
        "DE" | "GB" | "IE" => 22,
        "AT" | "LU" | "LT" | "EE" | "KZ" => 20,
        "CH" | "LI" | "LV" => 21,
        "NL" => 18,
        "BE" => 16,
        "DK" | "FI" | "FO" | "GL" => 18,
        "NO" => 15,
        "SE" | "ES" | "CZ" | "SK" | "RO" | "TN" => 24,
        "FR" | "IT" | "MC" | "SM" => 27,
        "PL" | "HU" | "CY" => 28,
        "PT" => 25,
        "GR" => 27,
        "SI" => 19,
        "HR" => 21,
        "TR" => 26,
        _ => return None,
    })
}

/// ISO 13616: the first four characters move to the end, letters become 10 to 35, the number mod 97 must be 1.
pub fn iban_checksum_ok(iban: &str) -> bool {
    let b = iban.as_bytes();
    if b.len() < 15 || b.len() > 34 || !b[..2].iter().all(u8::is_ascii_uppercase) || !b[2..4].iter().all(u8::is_ascii_digit)
        || !b.iter().all(|c| c.is_ascii_digit() || c.is_ascii_uppercase()) {
        return false;
    }
    let mut rest: u32 = 0;
    for &c in b[4..].iter().chain(b[..4].iter()) {
        if c.is_ascii_digit() {
            rest = (rest * 10 + (c - b'0') as u32) % 97;
        } else {
            rest = (rest * 100 + (c - b'A' + 10) as u32) % 97;
        }
    }
    rest == 1
}

fn iban_ok(compact: &str) -> bool {
    match iban_length(&compact[..2]) {
        Some(n) => compact.len() == n && iban_checksum_ok(compact),
        None => iban_checksum_ok(compact),
    }
}

/// The word "IBAN" or "Konto" shortly before: then an IBAN-shaped string counts even with a wrong check sum.
fn iban_context(text: &str, start: usize) -> bool {
    let from = floor(text, start.saturating_sub(24));
    let before = text[from..start].to_lowercase();
    before.contains("iban") || before.contains("konto")
}

fn ibans(text: &str, taken: &mut Taken) {
    static ONE: OnceLock<Regex> = OnceLock::new();
    static GROUPS: OnceLock<Regex> = OnceLock::new();
    for (s, e) in cores(text) {
        let w = &text[s..e];
        if re(&ONE, r"^[A-Z]{2}\d{2}[A-Z0-9]{11,30}$").is_match(w) && (iban_ok(w) || iban_context(text, s)) {
            taken.add(s, e, "iban");
        }
    }
    // in groups of four: DE89 3704 0044 0532 0130 00
    for m in re(&GROUPS, r"\b[A-Z]{2}\d{2}(?: [A-Z0-9]{1,4}){2,8}").find_iter(text) {
        let parts: Vec<(usize, &str)> = {
            let mut v = Vec::new();
            let mut at = m.start();
            for p in m.as_str().split(' ') {
                v.push((at, p));
                at += p.len() + 1;
            }
            v
        };
        let mut compact = String::new();
        let mut best: Option<usize> = None;
        let mut loose: Option<usize> = None;
        let want = iban_length(&m.as_str()[..2]);
        for (k, (at, p)) in parts.iter().enumerate() {
            compact.push_str(p);
            let end = at + p.len();
            let boundary = text[end..].chars().next().map(|c| !c.is_ascii_alphanumeric()).unwrap_or(true);
            if !boundary || k == 0 || compact.len() > 34 {
                continue;
            }
            if iban_ok(&compact) {
                best = Some(end);
            }
            let fits = match want { Some(n) => compact.len() == n, None => compact.len() >= 15 };
            if fits {
                loose = Some(end);
            }
        }
        let end = best.or_else(|| if iban_context(text, m.start()) { loose } else { None });
        if let Some(end) = end {
            taken.add(m.start(), end, "iban");
        }
    }
}

fn floor(text: &str, mut i: usize) -> usize {
    while i > 0 && !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// The text of the same line before `at`, at most `max` bytes.
fn line_before(text: &str, at: usize, max: usize) -> &str {
    let line_start = text[..at].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let from = floor(text, at.saturating_sub(max)).max(line_start);
    &text[from..at]
}

// ---------------------------------------------------------------- phone

fn phones(text: &str, taken: &mut Taken) {
    static PHONE: OnceLock<Regex> = OnceLock::new();
    static CONTEXT: OnceLock<Regex> = OnceLock::new();
    static NUMBER: OnceLock<Regex> = OnceLock::new();
    let phone = re(&PHONE, r"(?:(?:\+|\b00)\d{2}[ /-]?(?:\(0\) ?)?\d{2,5}|\(0\d{2,5}\)|\b0\d{2,5})(?:[ /-]?\d{2,}){1,4}(?:-\d{1,5})?");
    let context = re(&CONTEXT, r"(?i)(?:tel|telefon|fon|phone|mobil|handy|fax|rufnummer|erreichbar|anrufen|durchwahl|whatsapp)");
    let number = re(&NUMBER, r"(?i)(?:nr|nummer|art|bestell|kunden|rechnung|az|aktenzeichen|vertrag|personal|auftrag|beleg|ticket|vorgang|order|id)\.?\s*:?\s*$");
    for m in phone.find_iter(text) {
        let digits = m.as_str().chars().filter(|c| c.is_ascii_digit()).count();
        if !(9..=15).contains(&digits) {
            continue;
        }
        if text[..m.start()].chars().next_back().map(|c| c.is_alphanumeric() || c == '/' || c == '.').unwrap_or(false) {
            continue;
        }
        if text[m.end()..].chars().next().map(|c| c.is_alphanumeric()).unwrap_or(false) {
            continue;
        }
        let before = line_before(text, m.start(), 30);
        let has_context = context.is_match(before);
        let separated = m.as_str().contains([' ', '/', '-', '(']) || m.as_str().starts_with('+');
        if !separated && !has_context {
            continue;
        }
        if number.is_match(before) && !has_context {
            continue;
        }
        taken.add(m.start(), m.end(), "phone");
    }
}

// ---------------------------------------------------------------- tax id

/// The German tax id (Steuer-Identifikationsnummer): 11 digits, the first not 0, the last a check digit by ISO 7064
/// MOD 11,10.
pub fn tax_id_ok(digits: &str) -> bool {
    let d: Vec<u32> = digits.chars().filter_map(|c| c.to_digit(10)).collect();
    if d.len() != 11 || d[0] == 0 || digits.chars().any(|c| !c.is_ascii_digit()) {
        return false;
    }
    let mut product = 10;
    for &x in &d[..10] {
        let mut sum = (x + product) % 10;
        if sum == 0 {
            sum = 10;
        }
        product = (sum * 2) % 11;
    }
    let check = (11 - product) % 10;
    check == d[10]
}

fn tax_ids(text: &str, taken: &mut Taken) {
    static TAX: OnceLock<Regex> = OnceLock::new();
    static CONTEXT: OnceLock<Regex> = OnceLock::new();
    let context = re(&CONTEXT, r"(?i)(?:steuer|idnr|id-nr|identifikations|tin\b)");
    for m in re(&TAX, r"\b[1-9]\d(?: ?\d{3}){3}\b").find_iter(text) {
        let compact: String = m.as_str().chars().filter(|c| c.is_ascii_digit()).collect();
        if !tax_id_ok(&compact) {
            continue;
        }
        let grouped = m.as_str().contains(' ');
        if grouped || context.is_match(line_before(text, m.start(), 60)) {
            taken.add(m.start(), m.end(), "tax_id");
        }
    }
}

// ---------------------------------------------------------------- birth date

fn birth_dates(text: &str, taken: &mut Taken) {
    static DATE: OnceLock<Regex> = OnceLock::new();
    static CONTEXT: OnceLock<Regex> = OnceLock::new();
    let date = re(&DATE, r"\b(?:\d{1,2}\. ?\d{1,2}\. ?(?:19|20)\d{2}|\d{1,2}\. ?(?:Januar|Februar|März|Maerz|April|Mai|Juni|Juli|August|September|Oktober|November|Dezember|Jan\.|Feb\.|Mär\.|Apr\.|Jun\.|Jul\.|Aug\.|Sep\.|Sept\.|Okt\.|Nov\.|Dez\.) (?:19|20)\d{2}|(?:19|20)\d{2}-\d{2}-\d{2})\b");
    let context = re(&CONTEXT, r"(?i)(?:geb\.|geboren|geburtsdatum|geburtstag|geb\.-datum|\bgeb\b|\*)[^\n]{0,14}$");
    for m in date.find_iter(text) {
        if context.is_match(line_before(text, m.start(), 40)) {
            taken.add(m.start(), m.end(), "birth_date");
        }
    }
}

// ---------------------------------------------------------------- postal address

fn addresses(text: &str, taken: &mut Taken) {
    static SUFFIX: OnceLock<Regex> = OnceLock::new();
    static WORDS: OnceLock<Regex> = OnceLock::new();
    static PREFIX: OnceLock<Regex> = OnceLock::new();
    static PLZ: OnceLock<Regex> = OnceLock::new();
    const NO: &str = r"\d{1,3} ?[a-zA-Z]?(?: ?[-/] ?\d{1,3} ?[a-zA-Z]?)?";
    let suffix = re(&SUFFIX, &format!(r"\b(?:[A-ZÄÖÜ][a-zäöüß]+[ -]){{0,2}}[A-ZÄÖÜ][a-zäöüß]*(?:straße|strasse|str\.|weg|gasse|platz|allee|ring|damm|ufer|chaussee|steig|pfad|markt|wall|graben|stieg|twiete|kamp|hof|park|garten|feld|berg|tor|brücke|grund|anger|winkel|höhe) {}", NO));
    let words = re(&WORDS, &format!(r"\b(?:[A-ZÄÖÜ][a-zäöüß]+[ -]){{1,3}}(?:Straße|Strasse|Str\.|Weg|Gasse|Platz|Allee|Ring|Damm|Ufer|Chaussee|Markt|Steig|Pfad|Wall|Graben) {}", NO));
    let prefix = re(&PREFIX, &format!(r"\b(?:Am|An der|An den|An dem|Im|In der|In den|Zum|Zur|Auf der|Auf dem|Auf den|Unter den|Hinter der|Hinter dem|Vor dem|Bei der|Alte|Alter|Altes|Neue|Neuer|Neues|Großer|Kleiner|Obere|Untere|Obere|Untere) (?:[A-ZÄÖÜ][a-zäöüß]+ ){{0,2}}[A-ZÄÖÜ][a-zäöüß]+ {}", NO));
    for (pattern, strict) in [(suffix, false), (words, false), (prefix, true)] {
        for m in pattern.find_iter(text) {
            let mut end = m.end();
            // a house number never runs into the next word: "Lindenweg 4 in" keeps "4"
            while end > m.start() && text[..end].ends_with(|c: char| c.is_ascii_alphabetic())
                && text[end..].chars().next().map(|c| c.is_alphabetic()).unwrap_or(false) {
                end -= 1;
            }
            let end = text[..end].trim_end().len();
            let after = text[end..].chars().next();
            if text[end..].starts_with(|c: char| c.is_ascii_digit()) {
                continue;
            }
            if strict && !matches!(after, None | Some(',' | ';' | '\n' | ')' | '\r' | '·' | '|')) && !text[end..].trim_start().starts_with(|c: char| c.is_ascii_digit()) {
                continue;
            }
            taken.add(m.start(), end, "address");
        }
    }
    let plz = re(&PLZ, r"(?m)(?:^|[,:;·|]|D-|\s{2}) *(\d{5}) ((?:Bad |Sankt |St\. )?[A-ZÄÖÜ][a-zäöüß]+(?:-[A-ZÄÖÜ][a-zäöüß]+)*(?: (?:am|an der|im|in der|ob der|vor der|bei) [A-ZÄÖÜ][a-zäöüß]+)?(?: \([A-ZÄÖÜ][a-zäöüß]+\))?)");
    const NOT_CITY: &[&str] = &["Euro", "Stück", "Mal", "Tage", "Kunden", "Mitarbeiter", "Einwohner", "Personen", "Seiten", "Zeichen", "Punkte", "Stunden", "Minuten", "Jahre", "Monate", "Bestellungen"];
    for c in plz.captures_iter(text) {
        let (code, city) = (c.get(1).unwrap(), c.get(2).unwrap());
        if NOT_CITY.contains(&city.as_str()) || code.as_str() < "01001" {
            continue;
        }
        taken.add(code.start(), city.end(), "address");
    }
}

// ---------------------------------------------------------------- names

/// Common German first names, also those of families from Turkey, Poland and Italy who live in Germany.
const FIRST: &[&str] = &[
    "Anna", "Maria", "Emma", "Mia", "Sofia", "Sophia", "Sophie", "Hannah", "Hanna", "Emilia", "Lina", "Lea", "Leonie", "Marie", "Lena", "Laura", "Julia",
    "Sarah", "Sara", "Lisa", "Katharina", "Johanna", "Clara", "Klara", "Charlotte", "Amelie", "Luisa", "Louisa", "Ella", "Ida", "Greta", "Frieda", "Mila",
    "Nele", "Lara", "Paula", "Alina", "Jana", "Jasmin", "Vanessa", "Nina", "Sandra", "Sabine", "Susanne", "Petra", "Andrea", "Claudia", "Nicole",
    "Stefanie", "Stephanie", "Kerstin", "Birgit", "Monika", "Ursula", "Renate", "Gabriele", "Karin", "Elke", "Heike", "Martina", "Christina", "Christine",
    "Kristina", "Daniela", "Anja", "Silke", "Tanja", "Melanie", "Simone", "Manuela", "Doris", "Ingrid", "Helga", "Brigitte", "Gisela", "Erika",
    "Elisabeth", "Barbara", "Angelika", "Cornelia", "Beate", "Bettina", "Carina", "Franziska", "Theresa", "Teresa", "Miriam", "Nadine", "Svenja",
    "Antonia", "Maja", "Pia", "Jule", "Merle", "Annika", "Anke", "Ines", "Iris", "Judith", "Kathrin", "Katrin", "Marion", "Natalie", "Patricia",
    "Ramona", "Regina", "Rita", "Ruth", "Sonja", "Ulrike", "Verena", "Viktoria", "Yvonne", "Zoe", "Elena", "Selina", "Isabel", "Helena", "Magdalena",
    "Emily", "Jessica", "Jennifer", "Anne", "Eva", "Carla", "Carolin", "Caroline", "Denise", "Diana", "Esther", "Gudrun", "Hildegard", "Irene",
    "Jacqueline", "Lieselotte", "Marlene", "Michaela", "Nora", "Olga", "Rebecca", "Saskia", "Tamara", "Tatjana", "Vera", "Wiebke",
    "Ben", "Paul", "Leon", "Finn", "Elias", "Jonas", "Luis", "Louis", "Noah", "Felix", "Lukas", "Lucas", "Maximilian", "Henry", "Emil", "Theo",
    "Anton", "Jakob", "Jacob", "David", "Moritz", "Niklas", "Nico", "Tim", "Tom", "Jan", "Julian", "Fabian", "Florian", "Philipp", "Simon", "Tobias",
    "Sebastian", "Alexander", "Daniel", "Michael", "Thomas", "Andreas", "Stefan", "Stephan", "Christian", "Markus", "Marcus", "Matthias", "Martin",
    "Jürgen", "Klaus", "Peter", "Wolfgang", "Uwe", "Dieter", "Bernd", "Ralf", "Rolf", "Werner", "Manfred", "Helmut", "Gerhard", "Günter", "Günther",
    "Horst", "Heinz", "Karl", "Carl", "Hans", "Joachim", "Jörg", "Dirk", "Holger", "Sven", "Torsten", "Thorsten", "Oliver", "Patrick", "Dennis",
    "Marco", "Mario", "Kai", "Lars", "Björn", "Kevin", "Marcel", "Benjamin", "Johannes", "Jonathan", "Leonard", "Konstantin", "Vincent", "Oskar",
    "Oscar", "Mats", "Matteo", "Erik", "Eric", "Robin", "Robert", "Richard", "Rainer", "Reinhard", "Norbert", "Hubert", "Georg", "Gregor", "Hendrik",
    "Henrik", "Malte", "Ole", "Hannes", "Till", "Timo", "Lennart", "Linus", "Luca", "Samuel", "Benedikt", "Dominik", "Christoph", "Jens", "Volker",
    "Axel", "Udo", "Achim", "Armin", "Detlef", "Egon", "Friedrich", "Fritz", "Gustav", "Herbert", "Hermann", "Heinrich", "Ingo", "Kurt", "Lothar",
    "Ludwig", "Otto", "Siegfried", "Walter", "Wilhelm", "Willi", "Mehmet", "Ahmet", "Ali", "Mustafa", "Murat", "Ayşe", "Fatma", "Elif", "Zeynep",
    "Emine", "Hasan", "Hüseyin", "Ibrahim", "Yusuf", "Emir", "Can", "Piotr", "Tomasz", "Katarzyna", "Anastasia", "Natalia", "Alessandro", "Giuseppe",
    "Francesca", "Sergej", "Andrej", "Igor", "Ivan", "Nikolai", "Max", "Frank", "Ernst", "Rosa", "Mark", "August", "Wolf",
];

/// First names that are also German words. They count only next to a listed surname or in a name context.
const AMBIGUOUS: &[&str] = &["Max", "Frank", "Ernst", "Rosa", "Mark", "August", "Wolf", "Can", "Ali", "Will", "Henry", "Emil"];

/// Short first names that are also words or abbreviations ("Jan." for January, "am Kai"). They never count alone.
const NEVER_ALONE: &[&str] = &["Jan", "Kai", "Tom", "Ben", "Ole", "Ida", "Mia", "Eva", "Tim", "Max"];

/// Common German surnames and a few common surnames of families from other countries.
const SURNAMES: &[&str] = &[
    "Müller", "Schmidt", "Schneider", "Fischer", "Weber", "Meyer", "Wagner", "Becker", "Schulz", "Hoffmann", "Schäfer", "Koch", "Bauer", "Richter",
    "Klein", "Wolf", "Schröder", "Neumann", "Schwarz", "Zimmermann", "Braun", "Krüger", "Hofmann", "Hartmann", "Lange", "Schmitt", "Werner",
    "Schmitz", "Krause", "Meier", "Lehmann", "Schmid", "Schulze", "Maier", "Köhler", "Herrmann", "König", "Walter", "Mayer", "Huber", "Kaiser",
    "Fuchs", "Peters", "Lang", "Scholz", "Möller", "Weiß", "Jung", "Hahn", "Schubert", "Vogel", "Friedrich", "Keller", "Günther", "Frank", "Berger",
    "Winkler", "Roth", "Beck", "Lorenz", "Baumann", "Franke", "Albrecht", "Schuster", "Simon", "Ludwig", "Böhm", "Winter", "Kraus", "Martin",
    "Schumacher", "Krämer", "Vogt", "Stein", "Jäger", "Otto", "Sommer", "Groß", "Seidel", "Heinrich", "Brandt", "Haas", "Schreiber", "Graf",
    "Schulte", "Dietrich", "Ziegler", "Kuhn", "Kühn", "Pohl", "Engel", "Horn", "Busch", "Bergmann", "Thomas", "Voigt", "Sauer", "Arnold", "Wolff",
    "Pfeiffer", "Berg", "Lindner", "Brinkmann", "Ernst", "Franz", "Hansen", "Jansen", "Kremer", "Lehmann", "Marx", "Nagel", "Petersen", "Ritter",
    "Schütz", "Stahl", "Thiel", "Ulrich", "Wendt", "Wirth", "Yilmaz", "Kaya", "Demir", "Şahin", "Çelik", "Yıldız", "Öztürk", "Nowak", "Kowalski",
    "Wiśniewski", "Lewandowski", "Nguyen", "Tran", "Popescu", "Ivanov", "Petrović", "Rossi", "Russo", "Esposito", "Bianchi",
];

/// Words that start with a capital letter in German text but are no names.
const STOP: &[&str] = &[
    "Der", "Die", "Das", "Den", "Dem", "Des", "Ein", "Eine", "Einen", "Einem", "Einer", "Sie", "Ihr", "Ihre", "Ihren", "Ihrem", "Ihrer", "Ihnen",
    "Wir", "Uns", "Unser", "Unsere", "Unseren", "Ich", "Mein", "Meine", "Er", "Es", "Man", "Wer", "Was", "Wie", "Wo", "Wann", "Warum", "Bitte",
    "Danke", "Vielen", "Dank", "Mit", "Und", "Oder", "Aber", "Auch", "Nur", "Noch", "Schon", "Sehr", "Hallo", "Liebe", "Lieber", "Herr", "Herrn",
    "Frau", "Damen", "Herren", "Team", "Kollegen", "Kolleginnen", "Kollege", "Kollegin", "Alle", "Zusammen", "Grüße", "Gruß", "Grüßen", "GmbH",
    "AG", "KG", "SE", "UG", "OHG", "Co", "Ltd", "Inc", "Straße", "Weg", "Platz", "Rechnung", "Vertrag", "Betreff", "Datum", "Seite", "Anlage",
    "Anlagen", "Tel", "Telefon", "Fax", "Mobil", "E-Mail", "Email", "Mail", "IBAN", "BIC", "Steuer", "Kundennummer", "Rechnungsnummer",
    "Personalnummer", "Abteilung", "Personalabteilung", "Buchhaltung", "Geschäftsführung", "Montag", "Dienstag", "Mittwoch", "Donnerstag",
    "Freitag", "Samstag", "Sonntag", "Januar", "Februar", "März", "April", "Mai", "Juni", "Juli", "September", "Oktober", "November",
    "Dezember", "Euro", "EUR", "Im", "In", "Am", "An", "Auf", "Für", "Von", "Vom", "Zum", "Zur", "Bei", "Nach", "Aus", "Über", "Unter", "Vor",
    "Hinter", "Neben", "Zwischen", "Gestern", "Heute", "Morgen", "Anbei", "Leider", "Gerne", "Gern", "Hiermit", "Wenn", "Falls", "Da", "Dass",
    "Damit", "Bis", "Ab", "Seit", "Nachfolgend", "Arbeitgeber", "Arbeitnehmer", "Arbeitnehmerin", "Mieter", "Vermieter", "Kunde", "Kundin",
    "Geschäftsführer", "Geschäftsführerin", "Mitarbeiter", "Mitarbeiterin", "Ansprechpartner", "Ansprechpartnerin", "Name", "Vorname",
    "Nachname", "Unterschrift", "Ort", "Anschrift", "Adresse", "Hinweis", "Hinweise", "Freundliche", "Freundlichen", "Beste", "Besten",
    "Viele", "Herzliche", "Herzlichen", "Schöne", "Guten", "Tag", "Morgen", "Abend", "Hi", "Hey", "Moin", "Servus", "Personal", "Zeugnis",
    "Kündigung", "Gehalt", "Termin", "Gespräch", "Einladung", "Bestätigung", "Angebot", "Auftrag", "Lieferung", "Zahlung", "Summe", "Gesamt",
    "Netto", "Brutto", "MwSt", "USt", "Pos", "Menge", "Preis", "Beschreibung", "Position", "Konto", "Bank", "Sparkasse", "Volksbank", "Zahlbar",
    "Fällig", "Bezahlt", "Offen", "Wichtig", "Info", "Information", "Support", "Service", "Vertrieb", "Einkauf", "Verkauf", "Leitung",
    "Abteilungsleiter", "Abteilungsleiterin", "Teamleiter", "Teamleiterin", "Dr", "Prof", "Dipl", "Ihres", "Ihrem", "Diese", "Dieser",
    "Dieses", "Jeder", "Jede", "Keine", "Kein", "Weitere", "Weiterer", "Neue", "Neuer", "Alte", "Alter", "Sachbearbeiter", "Sachbearbeiterin",
    "Vertreten", "Vertreter", "Inhaber", "Inhaberin", "Kontoinhaber", "Kontoinhaberin", "Absender", "Empfänger", "Kopie", "Cc", "Bcc",
    "PS", "Vorstand", "Aufsichtsrat", "Amtsgericht", "Registergericht", "Handelsregister", "Sitz", "Zuständig", "Ansprechperson", "Gez",
];

const PARTICLES: &[&str] = &["von", "van", "de", "der", "zu", "vom", "ten", "ter", "da", "di", "del", "du", "le", "la", "el", "al", "bin"];

const LEGAL: &[&str] = &["GmbH", "AG", "KG", "SE", "UG", "OHG", "mbH", "Co", "Ltd", "Inc", "e.V", "eG", "GbR", "PartG", "KGaA", "Holding"];

const NOUN_ENDINGS: &[&str] = &["ung", "heit", "keit", "schaft", "nummer", "tion", "ment", "ität", "nis", "tum", "datum", "vertrag", "rechnung",
                                "adresse", "straße", "abteilung", "leitung", "gespräch", "termin", "zeugnis", "bescheinigung"];

struct Lists {
    first: HashSet<&'static str>,
    ambiguous: HashSet<&'static str>,
    surnames: HashSet<&'static str>,
    stop: HashSet<&'static str>,
}

fn lists() -> &'static Lists {
    static LISTS: OnceLock<Lists> = OnceLock::new();
    LISTS.get_or_init(|| Lists {
        first: FIRST.iter().copied().collect(),
        ambiguous: AMBIGUOUS.iter().copied().collect(),
        surnames: SURNAMES.iter().copied().collect(),
        stop: STOP.iter().copied().collect(),
    })
}

/// Is this first name in the detector list? The factory uses it to split names into listed and unlisted.
pub fn listed_first_name(w: &str) -> bool {
    lists().first.contains(w)
}

pub fn listed_surname(w: &str) -> bool {
    lists().surnames.contains(w)
}

pub fn stop_word(w: &str) -> bool {
    lists().stop.contains(w)
}

#[derive(Debug)]
struct Tok<'a> {
    s: usize,
    e: usize,
    w: &'a str,
    line: usize,
    /// The token ends a group: `,`, `;`, `:`, `!`, `?`, `.` or a closing bracket follows the core.
    stop_after: bool,
}

fn tokens(text: &str) -> Vec<Tok<'_>> {
    static WORD: OnceLock<Regex> = OnceLock::new();
    let mut out = Vec::new();
    let (mut line, mut last) = (0usize, 0usize);
    for m in re(&WORD, r"\S+").find_iter(text) {
        line += text[last..m.start()].matches('\n').count();
        last = m.start();
        let w = m.as_str();
        let lead = w.len() - w.trim_start_matches(is_delim).len();
        let core = w.trim_start_matches(is_delim).trim_end_matches(|c: char| is_delim(c) || matches!(c, '.' | ':' | '!' | '?'));
        if core.is_empty() {
            continue;
        }
        let tail = &w[lead + core.len()..];
        out.push(Tok { s: m.start() + lead, e: m.start() + lead + core.len(), w: core, line,
                       stop_after: tail.contains([',', ';', ':', '!', '?', '.', ')', ']', '>', '"', '“', '”']) });
    }
    out
}

/// A word that can be part of a name: capital first letter, then letters, hyphens or an apostrophe, some lowercase.
fn name_shape(w: &str) -> bool {
    let mut chars = w.chars();
    let Some(first) = chars.next() else { return false };
    if !first.is_uppercase() || w.chars().count() < 2 {
        return false;
    }
    if !w.chars().all(|c| c.is_alphabetic() || matches!(c, '-' | '\'' | '’')) || !w.chars().any(|c| c.is_lowercase()) {
        return false;
    }
    w.split('-').all(|p| p.chars().next().map(|c| c.is_uppercase()).unwrap_or(false))
}

fn nameish(w: &str) -> bool {
    name_shape(w) && !lists().stop.contains(w) && !LEGAL.contains(&w)
}

fn noun_like(w: &str) -> bool {
    let l = w.to_lowercase();
    NOUN_ENDINGS.iter().any(|e| l.ends_with(e)) && !lists().surnames.contains(w)
}

/// From token `i`, the name words on the same line: up to `max` words, particles between them allowed. Empty when the
/// group is a company (a legal form follows) or a taken span stands in the way.
fn group(toks: &[Tok], i: usize, max: usize, taken: &Taken) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    let mut j = i;
    let mut words = 0;
    while j < toks.len() && toks[j].line == toks[i].line && words < max {
        let t = &toks[j];
        if !taken.free(t.s, t.e) {
            break;
        }
        if PARTICLES.contains(&t.w) {
            if words == 0 && !matches!(t.w, "von" | "van" | "zu" | "de") {
                break;
            }
            if j + 1 < toks.len() && toks[j + 1].line == t.line && (nameish(toks[j + 1].w) || PARTICLES.contains(&toks[j + 1].w)) && !t.stop_after {
                out.push(j);
                j += 1;
                continue;
            }
            break;
        }
        if !nameish(t.w) || (words > 0 && noun_like(t.w)) {
            break;
        }
        out.push(j);
        words += 1;
        if t.stop_after {
            break;
        }
        j += 1;
    }
    while out.last().map(|&k| PARTICLES.contains(&toks[k].w)).unwrap_or(false) {
        out.pop();
    }
    if out.is_empty() {
        return out;
    }
    let next = out.last().unwrap() + 1;
    let company = out.iter().any(|&k| LEGAL.contains(&toks[k].w))
        || (next < toks.len() && toks[next].line == toks[i].line && (LEGAL.iter().any(|l| toks[next].w.starts_with(l)) || toks[next].w == "&"));
    if company {
        return Vec::new();
    }
    out
}

fn names(text: &str, taken: &mut Taken) {
    static TITLE: OnceLock<Regex> = OnceLock::new();
    static LABEL: OnceLock<Regex> = OnceLock::new();
    static ROLE: OnceLock<Regex> = OnceLock::new();
    static GREETING: OnceLock<Regex> = OnceLock::new();
    static CLOSING: OnceLock<Regex> = OnceLock::new();
    static LONE_TITLE: OnceLock<Regex> = OnceLock::new();
    let academic = r"(?:Dr\.|Prof\.|Dipl\.-[A-Za-zäöü]+\.?|Dr\. ?med\.|Dr\.-Ing\.|Mag\.|M\.\s?Sc\.|B\.\s?Sc\.)";
    let title = re(&TITLE, &format!(r"(?:\b(?:Herrn?|Frau|Hr\.|Fr\.)(?: {a})*|\b{a}(?: {a})*) $", a = academic));
    let label = re(&LABEL, r"(?i)(?:\b(?:name|vorname|nachname|ansprechpartner(?:in)?|kontoinhaber(?:in)?|mitarbeiter(?:in)?|arbeitnehmer(?:in)?|mieter(?:in)?|vermieter(?:in)?|bearbeiter(?:in)?|sachbearbeiter(?:in)?|empfänger(?:in)?|absender(?:in)?|von|an|cc|bcc|kopie|kontakt|inhaber(?:in)?|geschäftsführer(?:in)?|geschäftsführung|vorstand|unterschrift|antragsteller(?:in)?|bewerber(?:in)?|teilnehmer(?:in)?|kunde|kundin|verfasser(?:in)?|zuständig|ansprechperson|vertreter(?:in)?|beschäftigte[r]?|auszubildende[r]?|praktikant(?:in)?|vorgesetzte[r]?|unterzeichner(?:in)?|gesprächspartner(?:in)?|erstellt von|geprüft von|freigegeben von|teamleitung|leitung|projektleitung|verantwortlich)\s*:|\bi\. ?[AV]\.|\bz\. ?Hd\.?|\bgez\.|\bvertreten durch(?: den| die| ihre[n]?| seine[n]?)?(?: Geschäftsführer(?:in)?| Vorstand| Prokuristen| Prokuristin| Inhaber(?:in)?)?|\bper Adresse|\bc/o) *$");
    let role = re(&ROLE, r"(?:\b(?:Mitarbeiter(?:in)?|Kollege|Kollegin|Geschäftsführer(?:in)?|Sachbearbeiter(?:in)?|Ansprechpartner(?:in)?|Teamleiter(?:in)?|Abteilungsleiter(?:in)?|Personalleiter(?:in)?|Vorgesetzte[rn]?|Bewerber(?:in)?|Arbeitnehmer(?:in)?|Mieter(?:in)?|Vermieter(?:in)?|Kunde|Kundin|Prokurist(?:in)?|Inhaber(?:in)?|Auszubildende[rn]?|Praktikant(?:in)?|Nachfolger(?:in)?|Vertretung|Kontoinhaber(?:in)?)) $");
    let greeting = re(&GREETING, r"(?:^|[\s,])(?:Hallo|Hi|Hey|Moin|Liebe|Lieber|Servus|Guten Tag|Guten Morgen|Sehr geehrte|Sehr geehrter|Danke|Dank an|Grüße an|Gruß an) $");
    let closing = re(&CLOSING, r"(?i)^\s*(?:mit )?(?:freundlichen|besten|vielen|herzlichen|lieben|schönen|freundliche|beste|viele|herzliche|liebe|schöne|sonnige|kollegiale|kollegialen)? ?(?:grüßen|grüße|gruß|gruss|grüssen)\b[^\n]*$|^\s*(?:i\. ?[AV]\.|gez\.|ppa\.|Ihr|Ihre|Dein|Deine|Euer|Eure)\s*$");
    let lone_title = re(&LONE_TITLE, &format!(r"^\s*(?:Herrn?|Frau)(?: {a})*\s*$", a = academic));
    let l = lists();
    let toks = tokens(text);
    let line_starts: Vec<usize> = std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect();
    let line_text = |n: usize| -> &str {
        let s = line_starts[n];
        let e = line_starts.get(n + 1).map(|x| x - 1).unwrap_or(text.len());
        &text[s..e]
    };
    let prev_line = |n: usize| -> Option<&str> {
        (0..n).rev().map(line_text).find(|t| !t.trim().is_empty())
    };
    let mut hits: Vec<usize> = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let t = &toks[i];
        if !nameish(t.w) || !taken.free(t.s, t.e) {
            i += 1;
            continue;
        }
        let before = &text[line_starts[t.line]..t.s];
        let first_on_line = before.trim().is_empty();
        let mut found: Vec<usize> = Vec::new();
        if title.is_match(before) || label.is_match(before) {
            found = group(&toks, i, 4, taken);
        } else if first_on_line && t.line > 0 && prev_line(t.line).map(|p| lone_title.is_match(p)).unwrap_or(false) {
            found = group(&toks, i, 4, taken);
        } else if role.is_match(before) {
            let g = group(&toks, i, 3, taken);
            if g.iter().filter(|&&k| !PARTICLES.contains(&toks[k].w)).count() >= 2 || g.iter().any(|&k| l.first.contains(toks[k].w)) {
                found = g;
            }
        } else if greeting.is_match(before) {
            let g = group(&toks, i, 3, taken);
            let starts_ok = g.first().map(|&k| l.first.contains(toks[k].w) || l.surnames.contains(toks[k].w)).unwrap_or(false);
            if starts_ok || before.trim_end().ends_with(|c: char| c.is_alphabetic()) && matches!(before.trim().rsplit(' ').next(), Some("Hallo" | "Hi" | "Hey" | "Moin" | "Liebe" | "Lieber" | "Servus" | "Tag" | "Morgen")) {
                found = g;
            }
        }
        if found.is_empty() && first_on_line && t.line > 0 && prev_line(t.line).map(|p| closing.is_match(p)).unwrap_or(false) {
            // a signature: the whole line is one to four name words
            let line_toks: Vec<usize> = (i..toks.len()).take_while(|&k| toks[k].line == t.line).collect();
            let all_names = !line_toks.is_empty() && line_toks.len() <= 5
                && line_toks.iter().all(|&k| nameish(toks[k].w) || PARTICLES.contains(&toks[k].w) || matches!(toks[k].w, "Dr" | "Prof"));
            if all_names {
                found = group(&toks, i, 4, taken);
            }
        }
        if found.is_empty() && l.first.contains(t.w) {
            let g = group(&toks, i, 4, taken);
            let ambiguous = l.ambiguous.contains(t.w);
            if g.len() >= 2 && (!ambiguous || l.surnames.contains(toks[g[1]].w)) {
                found = g;
            } else if !ambiguous && !NEVER_ALONE.contains(&t.w) && t.w.chars().count() >= 3 {
                found = vec![i];
            }
        }
        if found.is_empty() && l.surnames.contains(t.w) && text[t.e..].starts_with(", ") {
            // "Berg, Anna" in forms and lists
            if let Some(n) = toks.get(i + 1) {
                if n.line == t.line && l.first.contains(n.w) {
                    found = vec![i, i + 1];
                }
            }
        }
        if found.is_empty() {
            i += 1;
            continue;
        }
        // "Nachname, Vorname" after a label
        let last = *found.last().unwrap();
        if found.len() == 1 && text[toks[last].e..].starts_with(", ") && (label.is_match(before) || title.is_match(before)) {
            if let Some(n) = toks.get(last + 1) {
                if n.line == toks[last].line && nameish(n.w) && (l.first.contains(n.w) || text[n.e..].trim_start_matches([' ', '\t']).starts_with(['\n', '\r']) || n.e == text.len()) {
                    found.push(last + 1);
                }
            }
        }
        for &k in &found {
            if taken.add(toks[k].s, toks[k].e, "name") {
                hits.push(k);
            }
        }
        i = found.last().unwrap() + 1;
    }
    // A name word found once is a name everywhere in the same text: "Frau Berg" in the address makes "Berg" in the
    // body a name too.
    let known: HashSet<&str> = hits.iter().map(|&k| toks[k].w).filter(|w| w.chars().count() >= 3 && !PARTICLES.contains(w)).collect();
    let mut more: Vec<usize> = Vec::new();
    for (k, t) in toks.iter().enumerate() {
        if known.contains(t.w) && taken.add(t.s, t.e, "name") {
            more.push(k);
        }
    }
    // A capitalised word right before a found surname is its first name: "Yara Lindqvist".
    for &k in hits.iter().chain(more.iter()) {
        let mut k = k;
        while k > 0 {
            let p = &toks[k - 1];
            if p.line != toks[k].line || p.stop_after || !nameish(p.w) || noun_like(p.w) {
                break;
            }
            let before = &text[line_starts[p.line]..p.s];
            if before.trim().is_empty() && !l.first.contains(p.w) {
                break;
            }
            if !taken.add(p.s, p.e, "name") {
                break;
            }
            k -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<(String, &'static str)> {
        spans(text).into_iter().map(|s| (text[s.start..s.end].to_string(), s.kind)).collect()
    }

    #[test]
    fn checksums_follow_the_standards() {
        assert!(iban_checksum_ok("DE89370400440532013000"));
        assert!(!iban_checksum_ok("DE89370400440532013001"));
        assert!(iban_checksum_ok("NL91ABNA0417164300"));
        assert!(iban_checksum_ok("AT611904300234573201"));
        assert!(iban_checksum_ok("CH9300762011623852957"));
        // the example id of the Bundeszentralamt fuer Steuern
        assert!(tax_id_ok("86095742719"));
        assert!(!tax_id_ok("86095742718"));
        assert!(!tax_id_ok("06095742719"));
    }

    #[test]
    fn finds_each_kind_in_a_letter() {
        let text = "Herrn\nJonas Weber\nLindenweg 4\n20457 Hamburg\n\nSehr geehrter Herr Weber,\nIhre IBAN DE89 3704 0044 0532 0130 00.\n\
Steuer-ID: 86 095 742 719, geboren am 03.07.1990.\nTel. 040 1234567-12, jonas.weber@example.org\n\nMit freundlichen Grüßen\nYara Lindqvist\n";
        let got = kinds(text);
        let want = [("Jonas", "name"), ("Weber", "name"), ("Lindenweg 4", "address"), ("20457 Hamburg", "address"), ("Weber", "name"),
                    ("DE89 3704 0044 0532 0130 00", "iban"), ("86 095 742 719", "tax_id"), ("03.07.1990", "birth_date"),
                    ("040 1234567-12", "phone"), ("jonas.weber@example.org", "email"), ("Yara", "name"), ("Lindqvist", "name")];
        let want: Vec<(String, &str)> = want.iter().map(|(a, b)| (a.to_string(), *b)).collect();
        assert_eq!(got, want);
    }

    #[test]
    fn leaves_business_numbers_and_companies_alone() {
        for plain in ["Rechnung Nr. RE-2026-004711 vom 12.03.2026", "Bestellnummer 4500123456", "Art.-Nr. 0815-4711-22",
                      "USt-IdNr.: DE123456789", "Amtsgericht Hamburg HRB 12345", "Nordwerk Logistik GmbH", "Summe: 1.234,56 EUR",
                      "Sehr geehrte Damen und Herren,", "Liebe Kolleginnen und Kollegen,", "Kundennummer 0045812734",
                      "Das Arbeitsverhältnis beginnt am 01.04.2026.", "10000 Stück geliefert", "Im Jahr 2025 stieg der Umsatz",
                      "max_tokens=4096", "fn password_hash(input: &str) -> String", "git@github.com:owner/repo.git"] {
            assert_eq!(kinds(plain), Vec::<(String, &str)>::new(), "{}", plain);
        }
    }
}
