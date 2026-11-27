//! The privacy file factory and its bench (work plan P24).
//!
//! `factory` writes synthetic German business documents (invoices, contracts, e-mails, HR letters) with every
//! personal item recorded as a gold span. The same seed gives the same bytes. No real person stands behind any item:
//! names are drawn at random from name lists and from invented surnames, IBANs and tax ids are random numbers with a
//! valid check digit, streets are invented.
//!
//! `bench` runs a detector over the documents and scores it per kind. The score uses only the masked text: `align`
//! finds which input bytes each `[masked:KIND]` replaced, so a model that returns masked text is scored the same way
//! as the rules.

use crate::pii::{iban_checksum_ok, listed_first_name, listed_surname, stop_word, tax_id_ok, Detector};
use serde_json::json;

pub const KINDS: &[&str] = &["name", "iban", "address", "phone", "email", "tax_id", "birth_date"];

#[derive(Clone, Debug)]
pub struct Gold {
    pub start: usize,
    pub end: usize,
    pub kind: &'static str,
    /// For names: `listed` when the mention holds a first name of the detector list or only a listed surname,
    /// `unlisted` otherwise. Empty for other kinds.
    pub tag: &'static str,
}

pub struct Doc {
    pub kind: &'static str,
    pub text: String,
    pub gold: Vec<Gold>,
}

impl Doc {
    fn new(kind: &'static str) -> Doc {
        Doc { kind, text: String::new(), gold: Vec::new() }
    }
    fn t(&mut self, s: &str) -> &mut Doc {
        self.text.push_str(s);
        self
    }
    fn p(&mut self, kind: &'static str, tag: &'static str, s: &str) -> &mut Doc {
        let start = self.text.len();
        self.text.push_str(s);
        self.gold.push(Gold { start, end: self.text.len(), kind, tag });
        self
    }
}

// ---------------------------------------------------------------- random numbers

/// SplitMix64: small, fast, and the same on every platform.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    pub fn range(&mut self, lo: usize, hi: usize) -> usize {
        lo + self.below(hi - lo + 1)
    }
    pub fn chance(&mut self, p: f64) -> bool {
        ((self.next() >> 11) as f64 / (1u64 << 53) as f64) < p
    }
    pub fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
    fn digits(&mut self, n: usize) -> String {
        (0..n).map(|_| char::from(b'0' + self.below(10) as u8)).collect()
    }
}

// ---------------------------------------------------------------- name and place lists of the factory

/// First names of the factory that are also in the detector list.
const FIRST_COMMON_F: &[&str] = &["Anna", "Maria", "Emma", "Sophie", "Hannah", "Lea", "Leonie", "Marie", "Lena", "Laura", "Julia", "Sarah", "Lisa",
    "Katharina", "Johanna", "Clara", "Charlotte", "Amelie", "Paula", "Nina", "Sandra", "Sabine", "Susanne", "Petra", "Andrea", "Claudia", "Nicole",
    "Stefanie", "Kerstin", "Birgit", "Monika", "Ursula", "Karin", "Heike", "Martina", "Christina", "Daniela", "Anja", "Tanja", "Melanie", "Simone",
    "Franziska", "Theresa", "Miriam", "Nadine", "Svenja", "Antonia", "Annika", "Judith", "Kathrin", "Ulrike", "Verena", "Elena", "Helena", "Ayşe",
    "Elif", "Zeynep", "Katarzyna", "Natalia", "Francesca", "Eva", "Nora", "Vera", "Carolin", "Michaela"];
const FIRST_COMMON_M: &[&str] = &["Paul", "Leon", "Finn", "Elias", "Jonas", "Luis", "Noah", "Felix", "Lukas", "Maximilian", "Theo", "Anton", "Jakob",
    "David", "Moritz", "Niklas", "Tim", "Jan", "Julian", "Fabian", "Florian", "Philipp", "Simon", "Tobias", "Sebastian", "Alexander", "Daniel",
    "Michael", "Thomas", "Andreas", "Stefan", "Christian", "Markus", "Matthias", "Martin", "Jürgen", "Klaus", "Peter", "Wolfgang", "Uwe", "Dieter",
    "Bernd", "Ralf", "Werner", "Helmut", "Gerhard", "Karl", "Hans", "Jörg", "Dirk", "Holger", "Sven", "Oliver", "Patrick", "Marco", "Kai", "Lars",
    "Björn", "Johannes", "Konstantin", "Oskar", "Erik", "Robert", "Georg", "Hendrik", "Malte", "Hannes", "Timo", "Lennart", "Dominik", "Christoph",
    "Mehmet", "Murat", "Mustafa", "Hasan", "Piotr", "Tomasz", "Giuseppe", "Sergej", "Ben"];
/// First names of the factory that the detector list does not hold.
const FIRST_RARE_F: &[&str] = &["Yara", "Ronja", "Smilla", "Thea", "Fenja", "Wiebka", "Imke", "Gesa", "Milena", "Vesna", "Oksana", "Olena", "Ottilie",
    "Adelheid", "Hedwig", "Liesel", "Malin", "Svea", "Talea", "Ingeborg", "Hiltrud", "Cosima", "Frauke", "Gunda", "Samira", "Nour", "Aylin", "Mei",
    "Linh", "Priya", "Chiara", "Giulia", "Agnieszka", "Małgorzata", "Dagny", "Runa", "Edda", "Henrike", "Jolanda", "Ilka"];
const FIRST_RARE_M: &[&str] = &["Tjark", "Fiete", "Hauke", "Kalle", "Mirko", "Goran", "Dragan", "Dmytro", "Bogdan", "Kasimir", "Ignaz", "Severin",
    "Quirin", "Korbinian", "Wendelin", "Arvid", "Bendix", "Joris", "Enno", "Hinnerk", "Ansgar", "Eike", "Folke", "Habib", "Karim", "Kerem", "Emre",
    "Burak", "Thanh", "Duc", "Arjun", "Rohan", "Lorenzo", "Pavel", "Krzysztof", "Wojciech", "Leander", "Xaver", "Gerrit", "Okke"];
const SURNAME_RARE: &[&str] = &["Lindqvist", "Okafor", "Haddad", "Ferreira", "Kovačević", "Tanaka", "Johansson", "Mbeki", "Szymańska", "Dimitriou",
    "Van Dalen", "Al-Masri", "Gonçalves", "Andersson", "Horvath", "Nakamura", "Oyelaran", "Brătescu"];
const SURNAME_HEAD: &[&str] = &["Hollen", "Wester", "Oster", "Brem", "Hag", "Asch", "Kett", "Röd", "Wend", "Fahl", "Stell", "Rieb", "Sieg", "Tann",
    "Ebel", "Gräv", "Hölt", "Lütt", "Mölt", "Niem", "Pütz", "Ried", "Schwen", "Teg", "Uhl", "Volk", "Wieg", "Zell", "Dörn", "Esch", "Grot", "Harms",
    "Iken", "Jessel", "Kolb", "Lamm", "Mens", "Nolt", "Ohl", "Prell", "Quast", "Rink", "Sell", "Trüb", "Ufer", "Vahl", "Wöll"];
const SURNAME_TAIL: &[&str] = &["mann", "meier", "er", "ing", "kamp", "hoff", "bach", "rath", "bruch", "horst", "feld", "haus", "brink", "linger",
    "huber", "ke", "ow", "itz", "sen", "inger", "egger", "ler", "dorf", "beck", "wede", "möller", "kötter"];
const SURNAME_COMMON: &[&str] = &["Müller", "Schmidt", "Schneider", "Fischer", "Weber", "Meyer", "Wagner", "Becker", "Schulz", "Hoffmann",
    "Schäfer", "Koch", "Bauer", "Richter", "Klein", "Schröder", "Neumann", "Schwarz", "Zimmermann", "Braun", "Krüger", "Hartmann", "Lange",
    "Schmitt", "Werner", "Krause", "Meier", "Lehmann", "Köhler", "Herrmann", "König", "Walter", "Huber", "Kaiser", "Fuchs", "Peters", "Scholz",
    "Möller", "Jung", "Hahn", "Vogel", "Keller", "Berger", "Roth", "Lorenz", "Baumann", "Albrecht", "Schuster", "Böhm", "Winter", "Jäger",
    "Sommer", "Brandt", "Haas", "Graf", "Ziegler", "Kuhn", "Engel", "Horn", "Busch", "Bergmann", "Arnold", "Berg", "Lindner", "Yilmaz", "Kaya",
    "Demir", "Öztürk", "Nowak", "Kowalski", "Nguyen", "Popescu", "Rossi"];

const STREET_HEAD: &[&str] = &["Linden", "Birken", "Eichen", "Ahorn", "Buchen", "Kastanien", "Rosen", "Tulpen", "Garten", "Wiesen", "Feld", "Wald",
    "Tal", "Bach", "Mühlen", "Kirch", "Schul", "Bahnhof", "Haupt", "Markt", "Schloss", "Burg", "Sonnen", "Stern", "Hafen", "Brunnen", "Kloster",
    "Mozart", "Goethe", "Schiller", "Beethoven", "Friedrich", "Luisen", "Kaiser", "Holunder", "Weiden", "Erlen", "Ulmen", "Fasanen", "Lerchen",
    "Amsel", "Drossel", "Finken", "Heide", "Moor", "Deich", "Rhein", "Elb", "Main"];
const STREET_TAIL: &[&str] = &["straße", "straße", "straße", "str.", "weg", "weg", "gasse", "platz", "allee", "ring", "damm", "ufer", "steig", "pfad"];
const STREET_ADJ: &[&str] = &["Berliner", "Hamburger", "Münchner", "Kölner", "Frankfurter", "Leipziger", "Dresdner", "Hannoversche", "Bremer",
    "Alte", "Neue", "Lange", "Kurze", "Obere", "Untere"];
const STREET_WORD: &[&str] = &["Straße", "Straße", "Allee", "Weg", "Ring", "Chaussee", "Landstraße"];
const STREET_PREFIX: &[&str] = &["Am", "An der", "Im", "Zur", "Zum", "Auf dem", "Unter den", "In den"];
const STREET_PREFIX_WORD: &[&str] = &["Lindenhof", "Mühle", "Wiesengrund", "Alten Post", "Kirchberg", "Eichen", "Gärten", "Sonnenhang", "Rosenhain",
    "Steinbruch", "Mühlbach", "Schlossgarten", "Weinberg"];

const CITIES: &[(&str, &str)] = &[("Berlin", "10"), ("Hamburg", "20"), ("München", "80"), ("Köln", "50"), ("Frankfurt am Main", "60"),
    ("Stuttgart", "70"), ("Düsseldorf", "40"), ("Leipzig", "04"), ("Dortmund", "44"), ("Essen", "45"), ("Bremen", "28"), ("Dresden", "01"),
    ("Hannover", "30"), ("Nürnberg", "90"), ("Duisburg", "47"), ("Bochum", "44"), ("Wuppertal", "42"), ("Bielefeld", "33"), ("Bonn", "53"),
    ("Münster", "48"), ("Mannheim", "68"), ("Karlsruhe", "76"), ("Augsburg", "86"), ("Wiesbaden", "65"), ("Mönchengladbach", "41"),
    ("Aachen", "52"), ("Braunschweig", "38"), ("Kiel", "24"), ("Chemnitz", "09"), ("Halle (Saale)", "06"), ("Magdeburg", "39"),
    ("Freiburg im Breisgau", "79"), ("Mainz", "55"), ("Lübeck", "23"), ("Erfurt", "99"), ("Rostock", "18"), ("Kassel", "34"), ("Potsdam", "14"),
    ("Saarbrücken", "66"), ("Oldenburg", "26"), ("Osnabrück", "49"), ("Heidelberg", "69"), ("Darmstadt", "64"), ("Regensburg", "93"),
    ("Würzburg", "97"), ("Ulm", "89"), ("Göttingen", "37"), ("Bad Homburg vor der Höhe", "61"), ("Garmisch-Partenkirchen", "82"), ("Jena", "07"),
    ("Neustadt an der Weinstraße", "67"), ("Lüneburg", "21"), ("Konstanz", "78"), ("Passau", "94"), ("Flensburg", "24")];

const COMPANY_HEAD: &[&str] = &["Nord", "Süd", "West", "Ost", "Alpen", "Rhein", "Elb", "Hanse", "Spree", "Isar", "Weser", "Neckar", "Ruhr", "Mosel",
    "Harz", "Taunus", "Eifel", "Küsten", "Brücken", "Falken"];
const COMPANY_MID: &[&str] = &["werk", "tec", "data", "bau", "log", "med", "plan", "form", "line", "soft", "netz", "kraft", "licht", "stahl"];
const COMPANY_TAIL: &[&str] = &[" Logistik", " Systeme", " Bau", " Consulting", " Handel", " Software", " Medizintechnik", " Energie", " Service", ""];
const LEGAL_FORM: &[&str] = &[" GmbH", " GmbH", " AG", " GmbH & Co. KG", " UG (haftungsbeschränkt)", " SE", " KG"];
const BANKS: &[(&str, &str)] = &[("Commerzbank", "COBADEFFXXX"), ("Deutsche Bank", "DEUTDEDBXXX"), ("Sparkasse", "SPKHDE2HXXX"),
    ("Volksbank", "GENODEF1XXX"), ("Postbank", "PBNKDEFFXXX"), ("GLS Bank", "GENODEM1GLS"), ("ING", "INGDDEFFXXX")];
const MAIL_HOSTS: &[&str] = &["gmx.de", "web.de", "t-online.de", "posteo.de", "mailbox.org", "gmail.com", "outlook.de", "freenet.de"];
const MONTHS: &[&str] = &["Januar", "Februar", "März", "April", "Mai", "Juni", "Juli", "August", "September", "Oktober", "November", "Dezember"];
const JOBS: &[(&str, &str)] = &[("Sachbearbeiter", "Sachbearbeiterin"), ("Softwareentwickler", "Softwareentwicklerin"),
    ("Lagerist", "Lageristin"), ("Projektleiter", "Projektleiterin"), ("Buchhalter", "Buchhalterin"), ("Vertriebsmitarbeiter", "Vertriebsmitarbeiterin"),
    ("Elektroniker", "Elektronikerin"), ("Kaufmann für Büromanagement", "Kauffrau für Büromanagement")];
const PRODUCTS: &[&str] = &["Beratung (Stunden)", "Wartungsvertrag Q1", "Lizenz Standard", "Montage vor Ort", "Versandkosten", "Schulung Grundlagen",
    "Druckerpatronen schwarz", "Bürostuhl Modell Comfort", "Reinigung Büroflächen", "Webhosting Paket M", "Fahrtkosten", "Dokumentation",
    "Ersatzteil Pumpe P-200", "Serverwartung", "Übersetzung Deutsch-Englisch"];

// ---------------------------------------------------------------- generated items

pub struct Person {
    pub first: String,
    pub last: String,
    pub female: bool,
    pub dr: bool,
    pub street: String,
    pub plz_city: String,
    pub city: String,
    pub phone: String,
    pub email: String,
    pub iban: String,
    pub tax: String,
    pub birth: String,
    first_listed: bool,
    last_listed: bool,
}

impl Person {
    fn full(&self) -> String {
        format!("{} {}", self.first, self.last)
    }
    fn tag_full(&self) -> &'static str {
        if self.first_listed { "listed" } else { "unlisted" }
    }
    fn tag_last(&self) -> &'static str {
        if self.last_listed { "listed" } else { "unlisted" }
    }
    fn sir(&self) -> &'static str {
        if self.female { "Frau" } else { "Herr" }
    }
    fn sir_acc(&self) -> &'static str {
        if self.female { "Frau" } else { "Herrn" }
    }
}

struct Company {
    name: String,
    street: String,
    plz_city: String,
    city: String,
    phone: String,
    domain: String,
    iban: String,
    bank: (&'static str, &'static str),
    ust: String,
    hrb: String,
}

fn invented_surname(rng: &mut Rng) -> String {
    loop {
        let s = format!("{}{}", rng.pick(SURNAME_HEAD), rng.pick(SURNAME_TAIL));
        if !listed_surname(&s) && !listed_first_name(&s) && !stop_word(&s) {
            return s;
        }
    }
}

fn street(rng: &mut Rng) -> String {
    let no = if rng.chance(0.15) { format!("{}{}", rng.range(1, 120), rng.pick(&["a", "b", "c"])) }
             else if rng.chance(0.05) { let a = rng.range(1, 60); format!("{}-{}", a, a + 2) }
             else { rng.range(1, 180).to_string() };
    match rng.below(10) {
        0..=5 => format!("{}{} {}", rng.pick(STREET_HEAD), rng.pick(STREET_TAIL), no),
        6 | 7 => format!("{} {} {}", rng.pick(STREET_ADJ), rng.pick(STREET_WORD), no),
        _ => format!("{} {} {}", rng.pick(STREET_PREFIX), rng.pick(STREET_PREFIX_WORD), no),
    }
}

fn plz_city(rng: &mut Rng) -> (String, String) {
    let (city, prefix) = *rng.pick(CITIES);
    (format!("{}{} {}", prefix, rng.digits(3), city), city.to_string())
}

const AREAS: &[&str] = &["30", "40", "89", "221", "69", "711", "211", "351", "341", "511", "911", "421", "228", "6221", "7531", "3641", "4131",
    "2104", "8031", "561"];
const MOBILE: &[&str] = &["151", "152", "157", "160", "162", "163", "170", "171", "172", "173", "174", "175", "176", "177", "178", "179", "1512"];

pub fn phone(rng: &mut Rng) -> String {
    let mobile = rng.chance(0.4);
    let area = if mobile { *rng.pick(MOBILE) } else { *rng.pick(AREAS) };
    let len = 10 - area.len().min(4) + rng.below(2);
    let sub = {
        let mut s = rng.range(1, 9).to_string();
        s.push_str(&rng.digits(len - 1));
        s
    };
    match rng.below(9) {
        0 => format!("+49 {} {}", area, sub),
        1 => format!("0{} {}", area, sub),
        2 => format!("0{}/{}", area, sub),
        3 => { let (a, b) = sub.split_at(3); format!("(0{}) {} {}", area, a, b) }
        4 => format!("0{}-{}", area, sub),
        5 => format!("+49 (0){} {}-{}", area, &sub[..sub.len() - 2], rng.range(0, 99)),
        6 => format!("0{} {}-{}", area, &sub[..sub.len() - 2], rng.range(10, 99)),
        7 => format!("0049 {} {}", area, sub),
        _ => { let (a, b) = sub.split_at(sub.len() / 2); format!("+49 {} {} {}", area, a, b) }
    }
}

fn ascii(s: &str) -> String {
    s.to_lowercase().replace('ä', "ae").replace('ö', "oe").replace('ü', "ue").replace('ß', "ss").replace(' ', "")
        .chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect()
}

fn iban_check(country: &str, bban: &str) -> String {
    let mut rest: u32 = 0;
    for c in bban.bytes().chain(country.bytes()).chain(b"00".iter().copied()) {
        if c.is_ascii_digit() {
            rest = (rest * 10 + (c - b'0') as u32) % 97;
        } else {
            rest = (rest * 100 + (c - b'A' + 10) as u32) % 97;
        }
    }
    format!("{}{:02}{}", country, 98 - rest, bban)
}

pub fn iban(rng: &mut Rng) -> String {
    let compact = match rng.below(20) {
        0 => iban_check("AT", &rng.digits(16)),
        1 => iban_check("CH", &rng.digits(17)),
        2 => {
            let bank: String = (0..4).map(|_| char::from(b'A' + rng.below(26) as u8)).collect();
            iban_check("NL", &format!("{}{}", bank, rng.digits(10)))
        }
        _ => iban_check("DE", &format!("{}{}", rng.range(1, 8), rng.digits(17))),
    };
    debug_assert!(iban_checksum_ok(&compact));
    if rng.chance(0.7) {
        compact.as_bytes().chunks(4).map(|c| std::str::from_utf8(c).unwrap()).collect::<Vec<_>>().join(" ")
    } else {
        compact
    }
}

/// A tax id with the digit rule of 2016: in the first ten digits one digit appears twice, one is missing, the
/// others once; the first digit is not 0; the last digit is the check digit.
pub fn tax_id(rng: &mut Rng) -> String {
    loop {
        let mut pool: Vec<u32> = (0..10).collect();
        let missing = rng.below(10);
        pool.remove(missing);
        let twice = pool[rng.below(9)];
        pool.push(twice);
        for i in (1..pool.len()).rev() {
            let j = rng.below(i + 1);
            pool.swap(i, j);
        }
        if pool[0] == 0 {
            continue;
        }
        let mut product = 10;
        for &x in &pool {
            let mut sum = (x + product) % 10;
            if sum == 0 {
                sum = 10;
            }
            product = (sum * 2) % 11;
        }
        let check = (11 - product) % 10;
        let s: String = pool.iter().chain(std::iter::once(&check)).map(|d| char::from_digit(*d, 10).unwrap()).collect();
        debug_assert!(tax_id_ok(&s));
        return if rng.chance(0.6) { format!("{} {} {} {}", &s[..2], &s[2..5], &s[5..8], &s[8..]) } else { s };
    }
}

fn date(rng: &mut Rng, y0: usize, y1: usize, long: bool) -> String {
    let (d, m, y) = (rng.range(1, 28), rng.range(1, 12), rng.range(y0, y1));
    if long {
        format!("{}. {} {}", d, MONTHS[m - 1], y)
    } else if rng.chance(0.15) {
        format!("{}.{}.{}", d, m, y)
    } else {
        format!("{:02}.{:02}.{}", d, m, y)
    }
}

fn person(rng: &mut Rng) -> Person {
    let female = rng.chance(0.5);
    let rare = rng.chance(0.2);
    let first = match (female, rare) {
        (true, false) => *rng.pick(FIRST_COMMON_F),
        (false, false) => *rng.pick(FIRST_COMMON_M),
        (true, true) => *rng.pick(FIRST_RARE_F),
        (false, true) => *rng.pick(FIRST_RARE_M),
    }.to_string();
    let last = match rng.below(20) {
        0..=8 => rng.pick(SURNAME_COMMON).to_string(),
        9 => rng.pick(SURNAME_RARE).to_string(),
        10 => format!("{}-{}", rng.pick(SURNAME_COMMON), invented_surname(rng)),
        _ => invented_surname(rng),
    };
    let (plz_city, city) = plz_city(rng);
    let email = match rng.below(4) {
        0 => format!("{}.{}@{}", ascii(&first), ascii(&last), rng.pick(MAIL_HOSTS)),
        1 => format!("{}.{}@{}", &ascii(&first)[..1], ascii(&last), rng.pick(MAIL_HOSTS)),
        2 => format!("{}{}{}@{}", ascii(&first), ascii(&last), rng.range(60, 99), rng.pick(MAIL_HOSTS)),
        _ => format!("{}_{}@{}", ascii(&last), rng.range(1, 999), rng.pick(MAIL_HOSTS)),
    };
    Person {
        first_listed: listed_first_name(&first),
        last_listed: listed_surname(&last),
        first, last, female, dr: rng.chance(0.08), street: street(rng), plz_city, city, phone: phone(rng), email, iban: iban(rng),
        tax: tax_id(rng), birth: { let long = rng.chance(0.3); date(rng, 1950, 2004, long) },
    }
}

fn company(rng: &mut Rng) -> Company {
    let base = format!("{}{}", rng.pick(COMPANY_HEAD), rng.pick(COMPANY_MID));
    let tail = *rng.pick(COMPANY_TAIL);
    let (plz_city, city) = plz_city(rng);
    Company {
        name: format!("{}{}{}", base, tail, rng.pick(LEGAL_FORM)),
        domain: format!("{}{}.de", ascii(&base), if tail.is_empty() { String::new() } else { format!("-{}", ascii(tail.trim())) }),
        street: street(rng), plz_city, city, phone: phone(rng), iban: iban(rng), bank: *rng.pick(BANKS),
        ust: format!("DE{}", rng.digits(9)), hrb: format!("HRB {}", rng.range(1000, 99999)),
    }
}

fn work_email(p: &Person, c: &Company) -> String {
    format!("{}.{}@{}", ascii(&p.first), ascii(&p.last), c.domain)
}

fn money(rng: &mut Rng, lo: usize, hi: usize) -> String {
    let v = rng.range(lo, hi);
    let euros = v / 100;
    let cents = v % 100;
    let mut s = euros.to_string();
    if euros >= 1000 {
        s = format!("{}.{:03}", euros / 1000, euros % 1000);
    }
    format!("{},{:02}", s, cents)
}

// ---------------------------------------------------------------- name mentions

fn full(d: &mut Doc, p: &Person) {
    d.p("name", p.tag_full(), &p.full());
}

/// "Frau Berg", "Herrn Dr. Weber"
fn formal(d: &mut Doc, p: &Person, accusative: bool) {
    d.t(if accusative { p.sir_acc() } else { p.sir() }).t(" ");
    if p.dr {
        d.t("Dr. ");
    }
    d.p("name", p.tag_last(), &p.last);
}

/// "Frau Anna Berg"
fn formal_full(d: &mut Doc, p: &Person, accusative: bool) {
    d.t(if accusative { p.sir_acc() } else { p.sir() }).t(" ");
    if p.dr {
        d.t("Dr. ");
    }
    full(d, p);
}

fn salutation(d: &mut Doc, rng: &mut Rng, p: &Person, informal: bool) {
    if informal {
        let hi = *rng.pick(&["Hallo", "Hi", "Liebe", "Moin"]);
        let hi = if hi == "Liebe" && !p.female { "Lieber" } else { hi };
        d.t(hi).t(" ").p("name", p.tag_full(), &p.first).t(",\n\n");
    } else {
        match rng.below(3) {
            0 => { d.t(if p.female { "Sehr geehrte " } else { "Sehr geehrter " }); formal(d, p, false); }
            1 => { d.t("Guten Tag "); formal(d, p, false); }
            _ => { d.t(if p.female { "Sehr geehrte " } else { "Sehr geehrter " }); formal_full(d, p, false); }
        }
        d.t(",\n\n");
    }
}

fn closing(d: &mut Doc, rng: &mut Rng, p: &Person, informal: bool) {
    let words = if informal { *rng.pick(&["Viele Grüße", "Beste Grüße", "Liebe Grüße", "Gruß", "Schöne Grüße"]) }
                else { *rng.pick(&["Mit freundlichen Grüßen", "Freundliche Grüße", "Mit besten Grüßen"]) };
    d.t(words).t("\n");
    if informal && rng.chance(0.5) {
        d.p("name", p.tag_full(), &p.first).t("\n");
    } else {
        if !informal && rng.chance(0.3) {
            d.t("i. A. ");
        }
        full(d, p);
        d.t("\n");
    }
}

fn address_block(d: &mut Doc, p: &Person) {
    d.t(p.sir_acc()).t("\n");
    if p.dr {
        d.t("Dr. ");
    }
    full(d, p);
    d.t("\n").p("address", "", &p.street).t("\n").p("address", "", &p.plz_city).t("\n");
}

fn company_line(d: &mut Doc, c: &Company) {
    d.t(&c.name).t(" · ").p("address", "", &c.street).t(" · ").p("address", "", &c.plz_city).t("\n");
}

fn footer(d: &mut Doc, rng: &mut Rng, c: &Company, boss: &Person) {
    d.t("\n").t(&c.name).t(" | Sitz: ").t(&c.city).t(" | Amtsgericht ").t(&c.city).t(" ").t(&c.hrb).t("\n");
    d.t(if boss.female { "Geschäftsführerin: " } else { "Geschäftsführer: " });
    if boss.dr {
        d.t("Dr. ");
    }
    full(d, boss);
    d.t(" | USt-IdNr.: ").t(&c.ust).t("\n");
    if rng.chance(0.5) {
        d.t("Tel. ").p("phone", "", &c.phone).t(" | ").p("email", "", &format!("info@{}", c.domain)).t(" | ").t(c.bank.0).t(" IBAN ")
            .p("iban", "", &c.iban).t(" | BIC ").t(c.bank.1).t("\n");
    }
}

// ---------------------------------------------------------------- the four document kinds

fn invoice(rng: &mut Rng) -> Doc {
    let mut d = Doc::new("invoice");
    let (c, customer, clerk, boss) = (company(rng), person(rng), person(rng), person(rng));
    company_line(&mut d, &c);
    d.t("\n");
    address_block(&mut d, &customer);
    let number = format!("RE-2026-{}", rng.digits(6));
    d.t("\n").t(&c.city).t(", ").t(&date(rng, 2026, 2026, false)).t("\n\n");
    d.t("Rechnung Nr. ").t(&number).t("\n");
    d.t("Kundennummer: ").t(&rng.range(10000, 99999).to_string()).t("\n");
    if rng.chance(0.5) {
        d.t("Bestellnummer: 45").t(&rng.digits(8)).t("\n");
    }
    d.t("Leistungszeitraum: ").t(MONTHS[rng.below(12)]).t(" 2026\n\n");
    salutation(&mut d, rng, &customer, false);
    d.t("wir berechnen Ihnen folgende Leistungen:\n\n");
    d.t("Pos. Beschreibung                    Menge  Einzelpreis  Gesamt\n");
    for i in 1..=rng.range(1, 4) {
        d.t(&format!("{}    {:<30} {:>5}  {:>10} EUR  {:>10} EUR\n", i, rng.pick(PRODUCTS), rng.range(1, 40), money(rng, 1000, 50000),
                     money(rng, 5000, 200000)));
    }
    d.t("\nNettobetrag: ").t(&money(rng, 10000, 900000)).t(" EUR\nzzgl. 19 % MwSt.: ").t(&money(rng, 1000, 170000))
        .t(" EUR\nRechnungsbetrag: ").t(&money(rng, 12000, 1000000)).t(" EUR\n\n");
    if rng.chance(0.4) {
        d.t("Der Betrag wird am ").t(&date(rng, 2026, 2026, false)).t(" per SEPA-Lastschrift von Ihrem Konto ").p("iban", "", &customer.iban)
            .t(" abgebucht (Mandatsreferenz M-").t(&rng.digits(7)).t(").\n");
    } else {
        d.t("Bitte überweisen Sie den Betrag bis zum ").t(&date(rng, 2026, 2026, false)).t(" auf unser Konto:\nIBAN: ").p("iban", "", &c.iban)
            .t("\nBIC: ").t(c.bank.1).t(" (").t(c.bank.0).t(")\nVerwendungszweck: ").t(&number).t("\n");
    }
    d.t("\nBei Fragen erreichen Sie ");
    match rng.below(3) {
        0 => { d.t(if clerk.female { "Ihre Ansprechpartnerin " } else { "Ihren Ansprechpartner " }); formal_full(&mut d, &clerk, true); }
        1 => { full(&mut d, &clerk); d.t(" aus der Buchhaltung"); }
        _ => { d.t("unsere Buchhaltung ("); full(&mut d, &clerk); d.t(")"); }
    }
    d.t(" unter ").p("phone", "", &clerk.phone).t(" oder per E-Mail an ").p("email", "", &work_email(&clerk, &c)).t(".\n\n");
    closing(&mut d, rng, &clerk, false);
    d.t(&c.name).t("\n");
    footer(&mut d, rng, &c, &boss);
    d
}

fn contract(rng: &mut Rng) -> Doc {
    let mut d = Doc::new("contract");
    let (c, a, b, boss) = (company(rng), person(rng), person(rng), person(rng));
    match rng.below(3) {
        0 => {
            d.t("Arbeitsvertrag\n\nzwischen der ").t(&c.name).t(", ").p("address", "", &c.street).t(", ").p("address", "", &c.plz_city).t(",\n");
            d.t(if boss.female { "vertreten durch die Geschäftsführerin " } else { "vertreten durch den Geschäftsführer " });
            if boss.dr {
                d.t("Dr. ");
            }
            full(&mut d, &boss);
            d.t(",\n- nachfolgend \"Arbeitgeber\" -\n\nund\n\n");
            formal_full(&mut d, &a, true);
            d.t(", geboren am ").p("birth_date", "", &a.birth).t(", wohnhaft ").p("address", "", &a.street).t(", ").p("address", "", &a.plz_city).t(",\n");
            d.t("- nachfolgend \"Arbeitnehmer\" -\n\n§ 1 Beginn und Tätigkeit\nDas Arbeitsverhältnis beginnt am ").t(&date(rng, 2026, 2027, false)).t(". ");
            formal(&mut d, &a, false);
            let job = rng.pick(JOBS);
            d.t(" wird als ").t(if a.female { job.1 } else { job.0 }).t(" eingestellt.\n\n§ 2 Probezeit\nDie ersten sechs Monate gelten als Probezeit.\n\n");
            d.t("§ 3 Vergütung\nDer Arbeitnehmer erhält ein monatliches Bruttogehalt von ").t(&money(rng, 280000, 750000))
                .t(" EUR. Die Zahlung erfolgt bargeldlos auf das Konto IBAN ").p("iban", "", &a.iban).t(".\n\n");
            d.t("§ 4 Steuer\nSteuer-ID: ").p("tax_id", "", &a.tax).t("\n\n§ 5 Urlaub\nDer Urlaubsanspruch beträgt 30 Arbeitstage im Kalenderjahr.\n\n");
            d.t("§ 6 Mitteilungspflichten\nÄnderungen der Anschrift oder der Telefonnummer (derzeit ").p("phone", "", &a.phone)
                .t(") teilt der Arbeitnehmer unverzüglich mit.\n\n");
            d.t(&c.city).t(", den ").t(&date(rng, 2026, 2026, false)).t("\n\n");
            d.t("______________________          ______________________\n");
            if boss.dr {
                d.t("Dr. ");
            }
            full(&mut d, &boss);
            d.t("                ");
            full(&mut d, &a);
            d.t("\n(Arbeitgeber)                    (Arbeitnehmer)\n");
        }
        1 => {
            d.t("Mietvertrag über Wohnraum\n\nVermieter: ");
            full(&mut d, &a);
            d.t(", ").p("address", "", &a.street).t(", ").p("address", "", &a.plz_city).t(", Tel. ").p("phone", "", &a.phone).t("\n");
            d.t("Mieter: ");
            full(&mut d, &b);
            d.t(", geb. ").p("birth_date", "", &b.birth).t(", bisher wohnhaft ").p("address", "", &b.street).t(", ").p("address", "", &b.plz_city).t("\n\n");
            let (flat_plz, _) = plz_city(rng);
            let flat = street(rng);
            d.t("§ 1 Mietsache\nVermietet wird die Wohnung im ").t(&rng.range(1, 5).to_string()).t(". Obergeschoss, ").p("address", "", &flat).t(", ")
                .p("address", "", &flat_plz).t(", bestehend aus ").t(&rng.range(1, 5).to_string()).t(" Zimmern, Küche und Bad, ca. ")
                .t(&rng.range(35, 140).to_string()).t(" m².\n\n");
            d.t("§ 2 Mietzeit\nDas Mietverhältnis beginnt am ").t(&date(rng, 2026, 2027, false)).t(" und läuft auf unbestimmte Zeit.\n\n");
            d.t("§ 3 Miete\nDie Nettokaltmiete beträgt ").t(&money(rng, 45000, 190000)).t(" EUR, die Vorauszahlung auf die Betriebskosten ")
                .t(&money(rng, 8000, 40000)).t(" EUR. Die Miete ist bis zum dritten Werktag eines Monats auf das Konto des Vermieters zu zahlen:\nIBAN ")
                .p("iban", "", &a.iban).t(", Kontoinhaber: ");
            full(&mut d, &a);
            d.t("\n\n§ 4 Kaution\nDer Mieter leistet eine Kaution in Höhe von drei Nettokaltmieten.\n\n");
            d.t(&a.city).t(", ").t(&date(rng, 2026, 2026, false)).t("\n\nUnterschrift Vermieter: ");
            full(&mut d, &a);
            d.t("\nUnterschrift Mieter: ");
            full(&mut d, &b);
            d.t("\n");
        }
        _ => {
            d.t("Dienstleistungsvertrag\n\nAuftraggeber: ").t(&c.name).t(", ").p("address", "", &c.street).t(", ").p("address", "", &c.plz_city).t("\n");
            d.t("Auftragnehmer: ");
            full(&mut d, &a);
            d.t(" (Einzelunternehmen), ").p("address", "", &a.street).t(", ").p("address", "", &a.plz_city).t("\n");
            d.t("E-Mail: ").p("email", "", &a.email).t(", Mobil: ").p("phone", "", &a.phone).t("\nSteuerliche Identifikationsnummer: ")
                .p("tax_id", "", &a.tax).t("\n\n");
            d.t("§ 1 Gegenstand\nDer Auftragnehmer erbringt für den Auftraggeber ").t(rng.pick(PRODUCTS)).t(" nach Absprache.\n\n");
            d.t("§ 2 Vergütung\nDie Vergütung beträgt ").t(&money(rng, 6000, 15000)).t(" EUR pro Stunde zzgl. gesetzlicher Umsatzsteuer. Rechnungen sind innerhalb von 14 Tagen auf das Konto ")
                .p("iban", "", &a.iban).t(" zu zahlen.\n\n");
            d.t("§ 3 Ansprechpartner\nAnsprechpartner beim Auftraggeber ist ");
            formal_full(&mut d, &b, false);
            d.t(", erreichbar unter ").p("phone", "", &b.phone).t(" und ").p("email", "", &work_email(&b, &c)).t(".\n\n");
            d.t("§ 4 Laufzeit\nDer Vertrag beginnt am ").t(&date(rng, 2026, 2027, false)).t(" und kann mit einer Frist von vier Wochen gekündigt werden.\n\n");
            d.t(&c.city).t(", ").t(&date(rng, 2026, 2026, false)).t("\n\n");
            if boss.dr {
                d.t("Dr. ");
            }
            full(&mut d, &boss);
            d.t(" (Geschäftsführung)\n");
            full(&mut d, &a);
            d.t("\n");
        }
    }
    d
}

fn email(rng: &mut Rng) -> Doc {
    let mut d = Doc::new("email");
    let (c, a, b, x, y) = (company(rng), person(rng), person(rng), person(rng), person(rng));
    let informal = rng.chance(0.6);
    d.t("Von: ");
    full(&mut d, &a);
    d.t(" <").p("email", "", &work_email(&a, &c)).t(">\nAn: ");
    full(&mut d, &b);
    d.t(" <").p("email", "", &if rng.chance(0.5) { b.email.clone() } else { work_email(&b, &c) }).t(">\n");
    if rng.chance(0.3) {
        d.t("Cc: ");
        full(&mut d, &x);
        d.t(" <").p("email", "", &work_email(&x, &c)).t(">\n");
    }
    let subject = *rng.pick(&["Rückfrage zur Rechnung", "Termin nächste Woche", "Neue Bankverbindung", "Urlaubsvertretung", "Umzug",
                             "Unterlagen für die Personalakte", "Lieferung verspätet", "Projektstand"]);
    d.t("Datum: ").t(&date(rng, 2026, 2026, false)).t(", ").t(&format!("{:02}:{:02}", rng.range(7, 19), rng.range(0, 59))).t("\nBetreff: ")
        .t(subject).t("\n\n");
    salutation(&mut d, rng, &b, informal);
    let mut parts: Vec<u8> = vec![0, 1, 2, 3, 4, 5, 6];
    for i in (1..parts.len()).rev() {
        let j = rng.below(i + 1);
        parts.swap(i, j);
    }
    for &part in parts.iter().take(rng.range(2, 4)) {
        match part {
            0 => {
                d.t("ich habe gestern mit ");
                if rng.chance(0.5) { full(&mut d, &x); } else { formal(&mut d, &x, false); }
                d.t(" gesprochen, ");
                if rng.chance(0.5) { d.t(if x.female { "sie" } else { "er" }); } else { d.p("name", x.tag_full(), &x.first); }
                d.t(" meldet sich noch heute bei ").t(if informal { "dir" } else { "Ihnen" }).t(".\n");
            }
            1 => {
                d.t("ich bin vom ").t(&date(rng, 2026, 2026, false)).t(" bis zum ").t(&date(rng, 2026, 2026, false)).t(" im Urlaub. Meine Vertretung ist ");
                full(&mut d, &y);
                d.t(" (").p("phone", "", &y.phone).t(", ").p("email", "", &work_email(&y, &c)).t(").\n");
            }
            2 => {
                d.t("meine neue Bankverbindung für die Reisekostenabrechnung lautet ").p("iban", "", &a.iban).t(". Kontoinhaber ist ");
                full(&mut d, &a);
                d.t(".\n");
            }
            3 => {
                d.t("ab dem ").t(&date(rng, 2026, 2026, false)).t(" wohne ich in der ").p("address", "", &a.street).t(", ").p("address", "", &a.plz_city)
                    .t(". Bitte ändert die Anschrift auch in der Personalakte.\n");
            }
            4 => {
                d.t("mobil erreichst du mich unter ").p("phone", "", &a.phone).t(", privat bin ich unter ").p("email", "", &a.email).t(" erreichbar.\n");
            }
            5 => {
                d.t("die Lieferung für ").t(&c.name).t(" ist verspätet, ").t(if rng.chance(0.5) { "Herr " } else { "Frau " });
                d.p("name", x.tag_last(), &x.last).t(" aus dem Lager klärt das mit der Spedition.\n");
            }
            _ => {
                d.t("für die Personalakte brauche ich noch dein Geburtsdatum (").p("birth_date", "", &a.birth).t(") bestätigt und die Steuer-ID ")
                    .p("tax_id", "", &a.tax).t(".\n");
            }
        }
    }
    d.t("\n");
    closing(&mut d, rng, &a, informal);
    if rng.chance(0.6) {
        d.t("\n--\n");
        full(&mut d, &a);
        d.t("\n").t(&c.name).t("\n").p("address", "", &c.street).t(", ").p("address", "", &c.plz_city).t("\nTel. ").p("phone", "", &a.phone).t("\n");
    }
    d
}

fn hr_letter(rng: &mut Rng) -> Doc {
    let mut d = Doc::new("hr");
    let (c, p, clerk, boss) = (company(rng), person(rng), person(rng), person(rng));
    d.t(&c.name).t("\nPersonalabteilung\n").p("address", "", &c.street).t("\n").p("address", "", &c.plz_city).t("\n\n");
    address_block(&mut d, &p);
    let long = rng.chance(0.3);
    d.t("\n").t(&c.city).t(", ").t(&date(rng, 2026, 2026, long)).t("\n\nPersonalnummer: ").t(&rng.digits(6)).t("\n");
    match rng.below(4) {
        0 => {
            d.t("Anpassung Ihres Gehalts\n\n");
            salutation(&mut d, rng, &p, false);
            d.t("wir freuen uns, Ihnen mitzuteilen, dass Ihr Bruttogehalt ab dem ").t(&date(rng, 2026, 2026, false)).t(" auf ")
                .t(&money(rng, 300000, 800000)).t(" EUR steigt. Die Auszahlung erfolgt wie bisher auf Ihr Konto ").p("iban", "", &p.iban).t(".\n\n");
        }
        1 => {
            d.t("Einladung zum Personalgespräch\n\n");
            salutation(&mut d, rng, &p, false);
            d.t("wir laden Sie am ").t(&date(rng, 2026, 2026, false)).t(" um 10:00 Uhr zu einem Gespräch ein. Teilnehmen werden ");
            formal_full(&mut d, &clerk, false);
            d.t(" und Ihre Vorgesetzte bzw. Ihr Vorgesetzter. Bitte bestätigen Sie den Termin unter ").p("phone", "", &clerk.phone).t(".\n\n");
        }
        2 => {
            d.t("Arbeitszeugnis\n\n");
            formal_full(&mut d, &p, false);
            d.t(", geboren am ").p("birth_date", "", &p.birth).t(" in ").t(&p.city).t(", war vom ").t(&date(rng, 2015, 2022, false)).t(" bis zum ")
                .t(&date(rng, 2023, 2026, false)).t(" in unserem Unternehmen beschäftigt. ");
            formal(&mut d, &p, false);
            d.t(" erledigte alle Aufgaben stets zu unserer vollsten Zufriedenheit. Wir danken ");
            formal(&mut d, &p, true);
            d.t(" für die Zusammenarbeit und wünschen ").t(if p.female { "ihr" } else { "ihm" }).t(" alles Gute.\n\n");
        }
        _ => {
            d.t("Prüfung Ihrer Stammdaten\n\n");
            salutation(&mut d, rng, &p, false);
            d.t("bitte prüfen Sie die bei uns hinterlegten Daten:\n\nName: ");
            if rng.chance(0.5) {
                d.p("name", p.tag_full(), &format!("{}, {}", p.last, p.first));
            } else {
                full(&mut d, &p);
            }
            d.t("\nGeburtsdatum: ").p("birth_date", "", &p.birth).t("\nAnschrift: ").p("address", "", &p.street).t(", ").p("address", "", &p.plz_city)
                .t("\nTelefon: ").p("phone", "", &p.phone).t("\nE-Mail: ").p("email", "", &p.email).t("\nSteuer-ID: ").p("tax_id", "", &p.tax)
                .t("\nIBAN: ").p("iban", "", &p.iban).t("\n\nÄnderungen senden Sie bitte bis zum ").t(&date(rng, 2026, 2026, false))
                .t(" an ").p("email", "", &work_email(&clerk, &c)).t(".\n\n");
        }
    }
    closing(&mut d, rng, &clerk, false);
    d.t(if clerk.female { "Personalreferentin\n" } else { "Personalreferent\n" });
    footer(&mut d, rng, &c, &boss);
    d
}

/// Document `i` of the run with `seed`. The same pair gives the same bytes on every machine.
pub fn document(seed: u64, i: usize) -> Doc {
    let mut rng = Rng::new(seed ^ (i as u64).wrapping_mul(0xD1B5_4A32_D192_ED03));
    rng.next();
    match i % 4 {
        0 => invoice(&mut rng),
        1 => contract(&mut rng),
        2 => email(&mut rng),
        _ => hr_letter(&mut rng),
    }
}

// ---------------------------------------------------------------- scoring

/// The spans a masker replaced, found from its output alone: (start, end, kind) in the input. The output must equal
/// the input except for `[masked:KIND]` pieces. Returns the spans and whether the whole output lined up.
///
/// The output splits into literal pieces and masks. Each mask ends where its following literal piece starts in the
/// input. A literal like "." can also occur inside the masked text ("a@b.de."), so the search tries the places in
/// order and backtracks when a later piece does not fit. The last piece must end the input. A step budget keeps a
/// pathological text from running long; past it the first fitting place wins.
pub fn align(input: &str, output: &str) -> (Vec<(usize, usize, String)>, bool) {
    const OPEN: &str = "[masked:";
    // pieces: literal, (kind, literal), (kind, literal), ...
    let mut kinds: Vec<String> = Vec::new();
    let mut lits: Vec<&str> = Vec::new();
    let mut j = 0;
    loop {
        let next = output[j..].find(OPEN).map(|k| j + k);
        let Some(at) = next else {
            lits.push(&output[j..]);
            break;
        };
        let Some(close) = output[at..].find(']') else {
            lits.push(&output[j..]);
            break;
        };
        lits.push(&output[j..at]);
        kinds.push(output[at + OPEN.len()..at + close].to_string());
        j = at + close + 1;
    }
    if !input.starts_with(lits[0]) {
        return (Vec::new(), false);
    }
    let n = kinds.len();
    if n == 0 {
        return (Vec::new(), input == output);
    }
    // candidates for the end of mask m, starting the search after `from`
    let candidates = |m: usize, start: usize| -> Vec<usize> {
        let lit = lits[m + 1];
        let first = start + input[start..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
        if first > input.len() {
            return Vec::new();
        }
        if m + 1 == n {
            // the last literal ends the input
            return if input.len() >= lit.len() && input.ends_with(lit) && input.len() - lit.len() >= first { vec![input.len() - lit.len()] } else { Vec::new() };
        }
        if lit.is_empty() {
            // two masks side by side: break at a space or at the end of a word
            return input[first..].char_indices().filter(|(_, c)| c.is_whitespace()).map(|(k, _)| first + k).take(8).collect();
        }
        input[first..].match_indices(lit).map(|(k, _)| first + k).take(64).collect()
    };
    // depth-first search: frame m holds the candidate ends of mask m and the next one to try
    let mut stack: Vec<(Vec<usize>, usize)> = vec![(candidates(0, lits[0].len()), 0)];
    let mut chosen: Vec<usize> = Vec::new();
    let mut budget = 200_000usize;
    while let Some((cands, pos)) = stack.last_mut() {
        if *pos >= cands.len() || budget == 0 {
            if budget == 0 {
                break;
            }
            stack.pop();
            continue;
        }
        budget -= 1;
        let end = cands[*pos];
        *pos += 1;
        let m = stack.len() - 1;
        chosen.truncate(m);
        chosen.push(end);
        if m + 1 == n {
            let mut spans = Vec::new();
            let mut s = lits[0].len();
            for (k, &e) in chosen.iter().enumerate() {
                spans.push((s, e, kinds[k].clone()));
                s = e + lits[k + 1].len();
            }
            return (spans, true);
        }
        let next_start = end + lits[m + 1].len();
        stack.push((candidates(m + 1, next_start), 0));
    }
    (Vec::new(), false)
}

#[derive(Default, Clone)]
pub struct Count {
    pub gold: usize,
    pub found: usize,
    pub masks: usize,
    pub true_masks: usize,
}

pub struct Score {
    pub per_kind: Vec<(String, Count)>,
    pub names_listed: (usize, usize),
    pub names_unlisted: (usize, usize),
    pub misaligned: usize,
    pub misses: Vec<String>,
    pub false_alarms: Vec<String>,
    pub docs: usize,
    pub micros: u128,
}

fn context(text: &str, s: usize, e: usize) -> String {
    let mut a = s.saturating_sub(40);
    while !text.is_char_boundary(a) {
        a -= 1;
    }
    let mut b = (e + 40).min(text.len());
    while !text.is_char_boundary(b) {
        b += 1;
    }
    format!("{}[[{}]]{}", &text[a..s], &text[s..e], &text[e..b]).replace('\n', "⏎")
}

/// Scores a detector on `count` documents of `seed`. A gold item counts as found when every letter and digit of it
/// lies inside some mask. A mask counts as true when it overlaps any gold item.
pub fn score(det: &dyn Detector, docs: impl Iterator<Item = Doc>, keep: usize) -> Score {
    let mut per: Vec<(String, Count)> = KINDS.iter().map(|k| (k.to_string(), Count::default())).collect();
    let (mut listed, mut unlisted) = ((0, 0), (0, 0));
    let (mut misaligned, mut misses, mut false_alarms) = (0usize, Vec::new(), Vec::new());
    let (mut micros, mut count) = (0u128, 0usize);
    for (i, doc) in docs.enumerate() {
        count += 1;
        let started = std::time::Instant::now();
        let out = det.mask(&doc.text);
        micros += started.elapsed().as_micros();
        let (masks, ok) = align(&doc.text, &out);
        if !ok {
            misaligned += 1;
        }
        let mut covered = vec![false; doc.text.len()];
        for (s, e, _) in &masks {
            for c in covered.iter_mut().take(*e).skip(*s) {
                *c = true;
            }
        }
        for g in &doc.gold {
            let found = doc.text[g.start..g.end].char_indices().filter(|(_, c)| c.is_alphanumeric()).all(|(k, _)| covered[g.start + k]);
            let slot = per.iter_mut().find(|(k, _)| k == g.kind).unwrap();
            slot.1.gold += 1;
            if found {
                slot.1.found += 1;
            } else if misses.len() < keep {
                misses.push(format!("{:<10} doc {:>4} {}: {}", g.kind, i, doc.kind, context(&doc.text, g.start, g.end)));
            }
            if g.kind == "name" {
                let t = if g.tag == "listed" { &mut listed } else { &mut unlisted };
                t.0 += 1;
                if found {
                    t.1 += 1;
                }
            }
        }
        for (s, e, kind) in &masks {
            let hit = doc.gold.iter().any(|g| *s < g.end && g.start < *e);
            let name = if KINDS.contains(&kind.as_str()) { kind.clone() } else { "other".to_string() };
            if !per.iter().any(|(k, _)| *k == name) {
                per.push((name.clone(), Count::default()));
            }
            let slot = per.iter_mut().find(|(k, _)| *k == name).unwrap();
            slot.1.masks += 1;
            if hit {
                slot.1.true_masks += 1;
            } else if false_alarms.len() < keep {
                false_alarms.push(format!("{:<10} doc {:>4} {}: {}", kind, i, doc.kind, context(&doc.text, *s, *e)));
            }
        }
    }
    Score { per_kind: per, names_listed: listed, names_unlisted: unlisted, misaligned, misses, false_alarms, docs: count, micros }
}

fn pct(a: usize, b: usize) -> String {
    if b == 0 { "-".to_string() } else { format!("{:.2} %", 100.0 * a as f64 / b as f64) }
}

pub fn report(s: &Score, det: &str, seed: u64) -> String {
    let mut out = format!("privacy bench: {} documents, seed {}, detector {}, {:.1} ms in the detector\n\n", s.docs, seed, det, s.micros as f64 / 1000.0);
    out.push_str(&format!("{:<11} {:>6} {:>6} {:>9} {:>6} {:>6} {:>10}\n", "kind", "gold", "found", "recall", "masks", "true", "precision"));
    let mut all = Count::default();
    for (k, c) in &s.per_kind {
        out.push_str(&format!("{:<11} {:>6} {:>6} {:>9} {:>6} {:>6} {:>10}\n", k, c.gold, c.found, pct(c.found, c.gold), c.masks, c.true_masks,
                              pct(c.true_masks, c.masks)));
        all.gold += c.gold;
        all.found += c.found;
        all.masks += c.masks;
        all.true_masks += c.true_masks;
    }
    out.push_str(&format!("{:<11} {:>6} {:>6} {:>9} {:>6} {:>6} {:>10}\n", "all", all.gold, all.found, pct(all.found, all.gold), all.masks,
                          all.true_masks, pct(all.true_masks, all.masks)));
    out.push_str(&format!("\nnames with a listed first name or surname: {} of {} ({})\nnames without:                             {} of {} ({})\n",
                          s.names_listed.1, s.names_listed.0, pct(s.names_listed.1, s.names_listed.0),
                          s.names_unlisted.1, s.names_unlisted.0, pct(s.names_unlisted.1, s.names_unlisted.0)));
    if s.misaligned > 0 {
        out.push_str(&format!("documents whose masked text did not line up: {}\n", s.misaligned));
    }
    if !s.misses.is_empty() {
        out.push_str("\nmissed:\n");
        for m in &s.misses {
            out.push_str(&format!("  {}\n", m));
        }
    }
    if !s.false_alarms.is_empty() {
        out.push_str("\nfalse alarms:\n");
        for m in &s.false_alarms {
            out.push_str(&format!("  {}\n", m));
        }
    }
    out
}

fn report_json(s: &Score, det: &str, seed: u64) -> String {
    let kinds: serde_json::Map<String, serde_json::Value> = s.per_kind.iter().map(|(k, c)| (k.clone(), json!({
        "gold": c.gold, "found": c.found, "masks": c.masks, "true_masks": c.true_masks}))).collect();
    serde_json::to_string_pretty(&json!({"detector": det, "seed": seed, "documents": s.docs, "kinds": kinds,
        "names_listed": [s.names_listed.1, s.names_listed.0], "names_unlisted": [s.names_unlisted.1, s.names_unlisted.0],
        "misaligned": s.misaligned, "detector_ms": s.micros as f64 / 1000.0})).unwrap()
}

// ---------------------------------------------------------------- documents written by others

/// Documents in a text file, split by lines of `=====`, with personal items marked as `⟦kind|text⟧`. `⟦iban⟧` and
/// `⟦tax_id⟧` stand for numbers that this function fills with valid random ones, because a writer rarely gets check
/// digits right. Lets a second author, a person or another model, write test documents the rules never saw.
pub fn load_marked(text: &str, seed: u64) -> Vec<Doc> {
    let mark = regex::Regex::new(r"⟦(name|address|phone|email|birth_date)\|([^⟧]*)⟧|⟦(iban|tax_id)⟧").unwrap();
    let mut rng = Rng::new(seed);
    let mut docs = Vec::new();
    for part in text.split("\n=====") {
        let part = part.trim_start_matches(['=', '\n']).trim();
        if part.is_empty() || !part.contains('⟦') {
            continue;
        }
        let mut d = Doc::new("marked");
        let mut last = 0;
        for c in mark.captures_iter(part) {
            let m = c.get(0).unwrap();
            d.t(&part[last..m.start()]);
            if let (Some(kind), Some(value)) = (c.get(1), c.get(2)) {
                let kind = KINDS.iter().find(|k| **k == kind.as_str()).copied().unwrap_or("name");
                let mut value = value.as_str().trim();
                if kind == "name" {
                    // titles belong outside a name mark; some writers put them inside
                    loop {
                        let v = value.trim_start();
                        let Some(rest) = ["Herrn ", "Herr ", "Frau ", "Dr. ", "Prof. ", "Familie "].iter().find_map(|t| v.strip_prefix(t)) else { break };
                        d.t(&v[..v.len() - rest.len()]);
                        value = rest;
                    }
                }
                let tag = if kind != "name" { "" }
                    else if value.split([' ', ',', '-']).any(|w| listed_first_name(w) || listed_surname(w)) { "listed" } else { "unlisted" };
                if value.is_empty() {
                    // an empty mark is a writer's slip: keep the text and no gold
                } else {
                    d.p(kind, tag, value);
                }
            } else if let Some(kind) = c.get(3) {
                if kind.as_str() == "iban" {
                    let v = iban(&mut rng);
                    d.p("iban", "", &v);
                } else {
                    let v = tax_id(&mut rng);
                    d.p("tax_id", "", &v);
                }
            }
            last = m.end();
        }
        d.t(&part[last..]);
        docs.push(d);
    }
    docs
}

// ---------------------------------------------------------------- detectors for the bench

/// What the level "pii" of the harness does without a model: the key rules, then the personal-data rules.
struct Harness;

impl Detector for Harness {
    fn name(&self) -> String {
        "rules".to_string()
    }
    fn mask(&self, text: &str) -> String {
        let (keys, _) = crate::privacy::mask(text);
        crate::privacy::mask_pii(&keys).0
    }
}

/// The rules, then the privacy model of `roles.filter`, the same chain as `tools::guard_count`.
struct WithModel<'a> {
    ctx: &'a crate::config::Ctx,
    model: serde_json::Value,
}

impl Detector for WithModel<'_> {
    fn name(&self) -> String {
        format!("rules+{}", self.model["model"].as_str().unwrap_or("model"))
    }
    fn mask(&self, text: &str) -> String {
        let floor = Harness.mask(text);
        match crate::privacy::model_redact(self.ctx, &self.model, &floor) {
            Ok((t, _)) => t,
            Err(_) => floor,
        }
    }
}

fn value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).map(String::as_str)
}

fn number(args: &[String], flag: &str, default: u64) -> Result<u64, String> {
    match value(args, flag) {
        Some(v) => v.parse().map_err(|_| format!("{} needs a number, not {}", flag, v)),
        None => Ok(default),
    }
}

const USAGE: &str = "usage: penelope privacy factory --out DIR [--seed N] [--count N]\n       penelope privacy bench [--seed N] [--count N] [--file MARKED.txt] [--model] [--misses N] [--json]\n       penelope privacy scan FILE...";

pub fn cli(ctx: &crate::config::Ctx, args: &[String]) -> Result<String, String> {
    let seed = number(args, "--seed", 42)?;
    let count = number(args, "--count", 1000)? as usize;
    match args.first().map(String::as_str).unwrap_or("") {
        "factory" => {
            let out = value(args, "--out").ok_or(USAGE)?;
            let dir = std::path::Path::new(out);
            std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {}", out, e))?;
            let mut gold = String::new();
            for i in 0..count {
                let doc = document(seed, i);
                let file = format!("{:04}-{}.txt", i, doc.kind);
                std::fs::write(dir.join(&file), &doc.text).map_err(|e| e.to_string())?;
                let spans: Vec<serde_json::Value> = doc.gold.iter().map(|g| json!({"start": g.start, "end": g.end, "kind": g.kind,
                    "text": &doc.text[g.start..g.end], "tag": g.tag})).collect();
                gold.push_str(&json!({"file": file, "kind": doc.kind, "spans": spans}).to_string());
                gold.push('\n');
            }
            std::fs::write(dir.join("gold.jsonl"), gold).map_err(|e| e.to_string())?;
            Ok(format!("{} documents and gold.jsonl written to {}\n", count, out))
        }
        "bench" => {
            let keep = number(args, "--misses", 0)? as usize;
            let docs: Vec<Doc> = match value(args, "--file") {
                Some(f) => load_marked(&std::fs::read_to_string(f).map_err(|e| format!("cannot read {}: {}", f, e))?, seed),
                None => (0..count).map(|i| document(seed, i)).collect(),
            };
            let s;
            let name;
            if args.iter().any(|a| a == "--model") {
                let model = crate::route::model_for(ctx, "filter").cloned().ok_or("no model in roles.filter")?;
                let det = WithModel { ctx, model };
                name = det.name();
                s = score(&det, docs.into_iter(), keep);
            } else {
                name = Harness.name();
                s = score(&Harness, docs.into_iter(), keep);
            }
            Ok(if args.iter().any(|a| a == "--json") { report_json(&s, &name, seed) + "\n" } else { report(&s, &name, seed) })
        }
        "scan" => {
            let mut out = String::new();
            let (mut lines, mut total) = (0usize, std::collections::BTreeMap::<String, usize>::new());
            for f in args[1..].iter().filter(|a| !a.starts_with("--")) {
                let Ok(text) = std::fs::read_to_string(f) else { continue };
                lines += text.lines().count();
                let masked = Harness.mask(&text);
                let (spans, _) = align(&text, &masked);
                for (s, e, k) in spans {
                    if !KINDS.contains(&k.as_str()) {
                        continue; // keys and secrets belong to the older rules of privacy.rs
                    }
                    *total.entry(k.clone()).or_default() += 1;
                    out.push_str(&format!("{}: {:<10} {}\n", f, k, context(&text, s, e)));
                }
            }
            let sum: usize = total.values().sum();
            out.push_str(&format!("\n{} masks in {} lines ({:.2} per 1,000 lines): {:?}\n", sum, lines, 1000.0 * sum as f64 / lines.max(1) as f64, total));
            Ok(out)
        }
        _ => Err(USAGE.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_factory_is_deterministic_and_its_gold_is_exact() {
        for i in 0..40 {
            let (a, b) = (document(42, i), document(42, i));
            assert_eq!(a.text, b.text);
            for g in &a.gold {
                let piece = &a.text[g.start..g.end];
                assert!(!piece.trim().is_empty() && piece.trim() == piece, "{:?}", piece);
                if g.kind == "iban" {
                    assert!(iban_checksum_ok(&piece.replace(' ', "")), "{}", piece);
                }
                if g.kind == "tax_id" {
                    assert!(tax_id_ok(&piece.replace(' ', "")), "{}", piece);
                }
            }
        }
        assert_ne!(document(42, 0).text, document(43, 0).text);
    }

    #[test]
    fn rare_names_of_the_factory_are_not_in_the_detector_lists() {
        for n in FIRST_RARE_F.iter().chain(FIRST_RARE_M) {
            assert!(!listed_first_name(n), "{}", n);
        }
        for n in FIRST_COMMON_F.iter().chain(FIRST_COMMON_M) {
            assert!(listed_first_name(n), "{}", n);
        }
    }

    #[test]
    fn align_recovers_the_masked_spans() {
        let input = "Hallo Anna Berg, IBAN DE89 3704 0044 0532 0130 00.\nTel. 040 123";
        let output = "Hallo [masked:name] [masked:name], IBAN [masked:iban].\nTel. 040 123";
        let (spans, ok) = align(input, output);
        assert!(ok);
        let got: Vec<&str> = spans.iter().map(|(s, e, _)| &input[*s..*e]).collect();
        assert_eq!(got, vec!["Anna", "Berg", "DE89 3704 0044 0532 0130 00"]);
        let (_, ok) = align(input, "Hallo [masked:name], something else");
        assert!(!ok);
    }

    #[test]
    fn align_backtracks_when_the_literal_occurs_inside_the_mask() {
        let (spans, ok) = align("Fragen an a@b.de.", "Fragen an [masked:email].");
        assert!(ok);
        assert_eq!(spans, vec![(10, 16, "email".to_string())]);
    }

    #[test]
    fn the_rules_reach_the_gate_on_the_factory() {
        let s = score(&Harness, (0..120).map(|i| document(7, i)), 0);
        for (k, c) in &s.per_kind {
            if k == "name" || k == "iban" {
                assert!(c.found * 100 >= c.gold * 98, "{} found {} of {}", k, c.found, c.gold);
            }
        }
        assert_eq!(s.misaligned, 0);
    }

    /// Documents written by three other models (DeepSeek, GLM, Kimi) with marked personal data. The rules were tuned
    /// on rounds 1 and 2 and on the Kimi set of round 3; the GLM set of round 3 was measured once with frozen rules
    /// (names 92.9 %). Together they are a regression floor now.
    #[test]
    fn the_rules_hold_on_documents_of_other_writers() {
        let files = [include_str!("../tests/data/privacy/round1-deepseek.txt"), include_str!("../tests/data/privacy/round1-glm.txt"),
                     include_str!("../tests/data/privacy/round2-kimi.txt"), include_str!("../tests/data/privacy/round2-deepseek.txt"),
                     include_str!("../tests/data/privacy/round3-kimi.txt"), include_str!("../tests/data/privacy/round3-glm.txt")];
        let docs: Vec<Doc> = files.iter().flat_map(|f| load_marked(f, 42)).collect();
        assert!(docs.len() >= 140, "{}", docs.len());
        let s = score(&Harness, docs.into_iter(), 0);
        let get = |k: &str| s.per_kind.iter().find(|(n, _)| n == k).unwrap().1.clone();
        let (name, iban) = (get("name"), get("iban"));
        assert_eq!(iban.found, iban.gold);
        assert!(name.found * 100 >= name.gold * 94, "names {} of {}", name.found, name.gold);
        let all: (usize, usize) = s.per_kind.iter().fold((0, 0), |a, (_, c)| (a.0 + c.true_masks, a.1 + c.masks));
        assert!(all.0 * 100 >= all.1 * 96, "precision {} of {}", all.0, all.1);
    }
}
