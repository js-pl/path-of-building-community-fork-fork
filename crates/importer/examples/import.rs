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
    let mut im = importer::Importer::new(&dir);
    let t0 = std::time::Instant::now();
    let tables = importer::mod_parser::import(&mut im, &raw).unwrap_or_else(|e| panic!("{e}"));
    println!("mod parser tables in {:?}", t0.elapsed());
    let mut funcs = 0;
    for (k, v) in tables.entries() {
        let t = v.as_table().unwrap();
        let f = t
            .entries()
            .iter()
            .filter(|(_, v)| matches!(v, lua::Value::Func(_)))
            .count();
        funcs += f;
        println!(
            "{} {} entries, {} functions",
            k.to_value().as_str().unwrap(),
            t.borrow().len_pairs(),
            f
        );
    }
    println!("{funcs} functions");
}
