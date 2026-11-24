//! The owl and the small status lines of the command line.

pub const OWL: &str = r#"
   ,___,
   {o,o}    Strix Forgery
   /)_)     "#;

pub fn banner(sub: &str) {
    println!("{}{}", OWL, sub);
    println!();
}

pub fn say(line: &str) {
    println!("{{o,o}} {}", line);
}

pub fn warn(line: &str) {
    eprintln!("{{>,<}} {}", line);
}
