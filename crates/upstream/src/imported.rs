//! Upstream's data files for the translated program: evaluated by the
//! importer from the submodule at run time, their functions replaced by
//! their translations in [`crate::data`].

use crate::host::DataSource;
use importer::eval;
use lua::env::Env;
use lua::{LResult, LuaError, Value};
use std::path::PathBuf;
use std::rc::{Rc, Weak};

/// Data files read from upstream's `src/` directory.
pub struct Imported {
    src_dir: PathBuf,
}

impl Imported {
    pub fn new(src_dir: impl Into<PathBuf>) -> Imported {
        Imported {
            src_dir: src_dir.into(),
        }
    }

    pub fn src_dir(&self) -> &std::path::Path {
        &self.src_dir
    }
}

/// The program's globals, as data files see them.
struct ProgramGlobals(Weak<Env>);

impl eval::Globals for ProgramGlobals {
    fn get(&self, name: &str) -> Value {
        self.0.upgrade().map(|e| e.global(name)).unwrap_or_default()
    }

    fn set(&self, name: &str, v: Value) {
        if let Some(e) = self.0.upgrade() {
            e.set(name, v);
        }
    }
}

impl DataSource for Imported {
    fn load(&self, env: &Rc<Env>, name: &str, args: &[Value]) -> LResult<Vec<Value>> {
        let rel = format!("{name}.lua");
        // Data files with code are translated whole
        if let Some(chunk) = crate::up::chunk(&format!("src/{rel}")) {
            return chunk(env, args);
        }
        let src = std::fs::read(self.src_dir.join(&rel))
            .map_err(|e| LuaError::msg(format!("LoadModule() error loading '{rel}': {e}")))?;
        load_data(env, rel, Rc::new(src), args)
    }

    fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.src_dir.join(path.trim_start_matches("./"))).ok()
    }
}

/// `LoadModule` of a data file (path relative to upstream's `src/`): a
/// file returning a function gives one that evaluates its body
pub fn load_data(
    env: &Rc<Env>,
    rel: String,
    src: Rc<Vec<u8>>,
    args: &[Value],
) -> LResult<Vec<Value>> {
    let wrapped = lua::parser::parse(&src).is_ok_and(|b| lua::ast::data_wrapper(&b).is_some());
    if !wrapped {
        return evaluate(env, &rel, &src, args);
    }
    let env = Rc::downgrade(env);
    let f = lua::rt::func(&rel.clone(), move |a| match env.upgrade() {
        Some(env) => evaluate(&env, &rel, &src, a),
        None => Ok(vec![]),
    });
    Ok(vec![f])
}

/// Evaluates a data file (path relative to upstream's `src/`) for the program.
pub fn evaluate(env: &Rc<Env>, rel: &str, src: &[u8], args: &[Value]) -> LResult<Vec<Value>> {
    let mut ev = eval::Env::new();
    ev.program = Some(Rc::new(ProgramGlobals(Rc::downgrade(env))));
    let fenv = env.clone();
    ev.make_function = Some(Rc::new(move |r, locals| {
        crate::data::make(&r.file, r.offset, &fenv, locals)
    }));
    ev.eval_file(rel, src, args.to_vec())
        .map_err(|e| LuaError::msg(e.to_string()))
}
