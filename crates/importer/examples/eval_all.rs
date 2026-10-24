//! Evaluates every data file with placeholder arguments; prints failures.
use importer::eval::Env;
use lua::{TableRef, Value};

fn main() {
    let src_dir = std::path::PathBuf::from(std::env::args().nth(1).expect("upstream src dir"));
    let mut files = vec![];
    let mut stack = vec![src_dir.join("Data")];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "lua") {
                files.push(p);
            }
        }
    }
    files.sort();
    let (mut ok, mut bad) = (0, 0);
    for f in files {
        let rel = f
            .strip_prefix(&src_dir)
            .unwrap()
            .to_string_lossy()
            .to_string();
        let mut env = Env::new();
        let mk = env.native("mod", |_| Ok(vec![Value::Table(TableRef::new())]));
        let src = std::fs::read(&f).unwrap();
        let t0 = std::time::Instant::now();
        let args = vec![Value::Table(TableRef::new()), mk.clone(), mk.clone(), mk];
        match env.eval_file(&rel, &src, args) {
            Ok(_) => ok += 1,
            Err(e) => {
                bad += 1;
                println!("{e}");
            }
        }
        let dt = t0.elapsed();
        if dt.as_millis() > 2000 {
            println!("slow: {rel} {dt:?}");
        }
    }
    println!("{ok} ok, {bad} failed");
}
