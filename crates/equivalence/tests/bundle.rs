//! The typed engine on its data read from a bundle (`session::bundle`,
//! what the browser loads) gives the answers it gives on the data made
//! from upstream's source, for builds with each timeless jewel. The
//! bundle starts with what starts the engine; the files a build asks for
//! (the timeless jewels' data and look-up tables, older trees) are added
//! when it reports them missing, as the browser's worker does.

use serde_json::json;
use session::Dispatcher;
use session::typed::TypedData;
use std::collections::BTreeMap;
use std::rc::Rc;

#[test]
fn bundled_timeless_jewels_match_source() {
    let src = equivalence::upstream_src();
    let (core, lazy) = session::bundle::typed_files(&src).unwrap_or_else(|e| panic!("{e}"));
    let lazy: BTreeMap<String, Vec<u8>> = lazy.into_iter().collect();
    let files = Rc::new(session::Files::new());
    for (path, bytes) in core {
        files.insert(path, bytes);
    }
    let bundled: &'static TypedData = Box::leak(Box::new(
        TypedData::from_files(files.clone()).unwrap_or_else(|e| panic!("{e}")),
    ));
    let source: &'static TypedData = Box::leak(Box::new(
        TypedData::from_source(&src).unwrap_or_else(|e| panic!("{e}")),
    ));
    // A build with each jewel and conqueror: the first variant of each
    let mut eq = equivalence::typed_build::TypedData::load();
    let mut seen = std::collections::HashSet::new();
    let builds: Vec<(String, String)> = equivalence::variants::timeless_variants(&mut eq)
        .into_iter()
        .filter(|(id, _)| seen.insert(id.split('-').nth(1).unwrap_or("").to_string()))
        .collect();
    drop(eq);
    assert_eq!(builds.len(), 11, "a build for each timeless jewel");

    let mut a = Dispatcher::typed(bundled);
    let mut b = Dispatcher::typed(source);
    let mut added = Vec::new();
    let mut failures = Vec::new();
    files.take_missing();
    for (id, xml) in &builds {
        let req = json!({"cmd": "loadXml", "xml": xml}).to_string();
        let mut r = a.request(&req).unwrap();
        for _ in 0..5 {
            if !r.starts_with("{\"error\"") {
                break;
            }
            let missing = files.take_missing();
            assert!(!missing.is_empty(), "{id}: {r}");
            for path in missing {
                let bytes = lazy
                    .get(&path)
                    .unwrap_or_else(|| panic!("{id}: {path} is not in the bundle"));
                files.insert(path.clone(), bytes.clone());
                added.push(path);
            }
            r = a.request(&req).unwrap();
        }
        assert!(!r.starts_with("{\"error\""), "{id}: {r}");
        let rb = b.request(&req).unwrap();
        assert!(!rb.starts_with("{\"error\""), "{id}: {rb}");
        for cmd in ["state", "outputs", "treeState"] {
            let req = json!({"cmd": cmd}).to_string();
            let (x, y) = (a.request(&req).unwrap(), b.request(&req).unwrap());
            if x != y {
                failures.push(format!("{id}: {cmd} differs"));
            }
        }
    }
    eprintln!("added on request: {}", added.join(", "));
    // Every look-up table was fetched when a build first needed it
    for ty in engine::timeless::TYPES {
        let path = session::bundle::timeless_lut_path(&ty.replace(' ', ""));
        assert!(added.contains(&path), "{path} was never asked for");
    }
    assert!(
        added.contains(&session::bundle::TIMELESS.to_string()),
        "the timeless jewels' data was never asked for"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
