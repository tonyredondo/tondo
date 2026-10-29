// Packs `data/names.txt`, the names of months and days in each language
// (standard library 14.3.5), which a program that formats dates carries.

use std::io::Write;

fn main() {
    println!("cargo::rerun-if-changed=data/names.txt");
    let names = std::fs::read("data/names.txt").expect("data/names.txt");
    let mut packed = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    packed.write_all(&names).expect("names packed");
    let out = std::path::Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("names.txt.gz");
    std::fs::write(out, packed.finish().expect("names packed")).expect("names written");
}
