//! The woven mark and the small status lines of the command line.

pub const LOOM: &str = r#"
   +=+=+=+
   +=+=+=+~    Penelope
   +=+=+=+     "#;

pub fn banner(sub: &str) {
    println!("{}{}", LOOM, sub);
    println!();
}

pub fn say(line: &str) {
    println!("[#|#] {}", line);
}

pub fn warn(line: &str) {
    eprintln!("[#!#] {}", line);
}
