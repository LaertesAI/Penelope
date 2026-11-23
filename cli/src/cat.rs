//! The mascot and the small status lines of the command line.

pub const CAT: &str = r#"
   /\_/\
  ( o.o )   Kate
   > ^ <    "#;

pub fn banner(sub: &str) {
    println!("{}{}", CAT, sub);
    println!();
}

pub fn say(line: &str) {
    println!("=^.^= {}", line);
}

pub fn warn(line: &str) {
    eprintln!("=>.<= {}", line);
}
