//! Runs the full import and prints a summary.
fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("upstream src dir"));
    let t0 = std::time::Instant::now();
    let raw = importer::import(&dir).unwrap_or_else(|e| panic!("{e}"));
    println!("imported in {:?}", t0.elapsed());
    for (k, v) in raw.data.entries() {
        let n = v.as_table().map(|t| t.borrow().len_pairs()).unwrap_or(0);
        println!(
            "data.{} {} {}",
            k.to_value()
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_default(),
            v.type_name(),
            n
        );
    }
    println!("globals: {}", raw.globals.borrow().len_pairs());
}
