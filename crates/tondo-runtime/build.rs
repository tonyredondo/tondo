// Packs `data/names.txt`, the names of months and days in each language
// (standard library 14.3.5), and `data/numbers.txt`, how each language
// writes numbers (10.6), which a program that formats dates or numbers
// carries.

use std::io::Write;

fn main() {
    for name in ["names.txt", "numbers.txt"] {
        println!("cargo::rerun-if-changed=data/{name}");
        let data = std::fs::read(format!("data/{name}")).expect("the data");
        let mut packed = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        packed.write_all(&data).expect("data packed");
        let out = std::path::Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join(format!("{name}.gz"));
        std::fs::write(out, packed.finish().expect("data packed")).expect("data written");
    }
}
