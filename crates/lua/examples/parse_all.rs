//! Parses every .lua file under a directory; prints failures.
fn main() {
    let root = std::env::args().nth(1).expect("directory");
    let mut ok = 0;
    let mut failed = 0;
    let mut stack = vec![std::path::PathBuf::from(root)];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "lua") {
                let src = std::fs::read(&path).unwrap();
                match lua::parser::parse(&src) {
                    Ok(_) => ok += 1,
                    Err(e) => {
                        failed += 1;
                        println!("{}: {e}", path.display());
                    }
                }
            }
        }
    }
    println!("{ok} parsed, {failed} failed");
}
