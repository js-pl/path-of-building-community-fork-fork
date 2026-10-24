//! Runs upstream's translated program headless: a port of upstream
//! `src/HeadlessWrapper.lua` (v2.67.2), which stubs the SimpleGraphic host
//! API, plus the module loader for translated modules and imported data.

use lua::env::Env;
use lua::rt::{self, err, first, func};
use lua::{LResult, LuaStr, TableRef, Value};
use std::cell::RefCell;
use std::rc::Rc;

/// Provides upstream's data files (`Data/...`, `TreeData/...`), which are
/// imported rather than translated: `LoadModule(name, args...)`.
pub trait DataSource {
    fn load(&self, env: &Rc<Env>, name: &str, args: &[Value]) -> LResult<Vec<Value>>;

    /// A file upstream opens for reading, relative to its `src/` directory.
    fn read_file(&self, path: &str) -> Option<Vec<u8>>;
}

/// The running program.
pub struct Host {
    pub env: Rc<Env>,
    main_object: Rc<RefCell<Value>>,
    callbacks: TableRef,
}

fn arg(a: &[Value], i: usize) -> Value {
    a.get(i).cloned().unwrap_or_default()
}

fn s(x: &str) -> Value {
    Value::str(x)
}

fn n(x: f64) -> Value {
    Value::Number(x)
}

fn none(_: &[Value]) -> LResult<Vec<Value>> {
    Ok(vec![])
}

/// Path of an upstream module in the repository, as the translated modules
/// are registered: `Modules/Main` -> `src/Modules/Main.lua`.
fn module_path(name: &str) -> String {
    let name = name.strip_suffix(".lua").unwrap_or(name);
    format!("src/{name}.lua")
}

impl Host {
    pub fn new(data: Rc<dyn DataSource>) -> Rc<Host> {
        let env = Env::new();
        let host = Rc::new(Host {
            env: env.clone(),
            main_object: Rc::new(RefCell::new(Value::Nil)),
            callbacks: TableRef::new(),
        });
        host.install(data);
        host
    }

    pub fn global(&self, name: &str) -> Value {
        self.env.global(name)
    }

    /// `runCallback(name, ...)`
    pub fn run_callback(&self, name: &str, args: &[Value]) -> LResult<Vec<Value>> {
        let f = rt::call(&self.global("runCallback"), &{
            let mut a = vec![s(name)];
            a.extend_from_slice(args);
            a
        })?;
        Ok(f)
    }

    fn install(self: &Rc<Self>, data: Rc<dyn DataSource>) {
        let env = &self.env;
        let set = |name: &str, f: Value| env.set(name, f);

        // Callbacks
        let callbacks = self.callbacks.clone();
        let main_object = self.main_object.clone();
        set(
            "runCallback",
            func("runCallback", move |a| {
                let name = arg(a, 0);
                let cb = callbacks.get(&name);
                if !cb.is_nil() {
                    return rt::call(&cb, a.get(1..).unwrap_or(&[]));
                }
                let main = main_object.borrow().clone();
                if main.truthy() {
                    let m = rt::index(&main, &name)?;
                    if !m.is_nil() {
                        let mut args = vec![main.clone()];
                        args.extend_from_slice(a.get(1..).unwrap_or(&[]));
                        return rt::call(&m, &args);
                    }
                }
                Ok(vec![])
            }),
        );
        let callbacks = self.callbacks.clone();
        set(
            "SetCallback",
            func("SetCallback", move |a| {
                callbacks.set(arg(a, 0), arg(a, 1));
                Ok(vec![])
            }),
        );
        let callbacks = self.callbacks.clone();
        set(
            "GetCallback",
            func("GetCallback", move |a| Ok(vec![callbacks.get(&arg(a, 0))])),
        );
        let main_object = self.main_object.clone();
        set(
            "SetMainObject",
            func("SetMainObject", move |a| {
                *main_object.borrow_mut() = arg(a, 0);
                Ok(vec![])
            }),
        );

        // Image handles
        let image_class = TableRef::new();
        image_class.set_str("__index", Value::Table(image_class.clone()));
        image_class.set_str(
            "Load",
            func("imageHandle:Load", |a| {
                rt::set_index(&arg(a, 0), s("valid"), Value::Bool(true))?;
                Ok(vec![])
            }),
        );
        image_class.set_str(
            "Unload",
            func("imageHandle:Unload", |a| {
                rt::set_index(&arg(a, 0), s("valid"), Value::Bool(false))?;
                Ok(vec![])
            }),
        );
        image_class.set_str(
            "IsValid",
            func("imageHandle:IsValid", |a| {
                Ok(vec![rt::index(&arg(a, 0), &s("valid"))?])
            }),
        );
        image_class.set_str(
            "SetLoadingPriority",
            func("imageHandle:SetLoadingPriority", none),
        );
        image_class.set_str(
            "ImageSize",
            func("imageHandle:ImageSize", |_| Ok(vec![n(1.0), n(1.0)])),
        );
        set(
            "NewImageHandle",
            func("NewImageHandle", move |_| {
                let h = TableRef::new();
                h.borrow_mut().meta = Some(image_class.clone());
                Ok(vec![Value::Table(h)])
            }),
        );

        // Rendering
        for name in [
            "RenderInit",
            "SetDPIScaleOverridePercent",
            "SetClearColor",
            "SetDrawLayer",
            "SetViewport",
            "SetDrawColor",
            "DrawImage",
            "DrawImageQuad",
            "DrawString",
            "SetWindowTitle",
            "SetCursorPos",
            "ShowCursor",
            "IsKeyDown",
            "Copy",
            "Paste",
            "MakeDir",
            "RemoveDir",
            "SetWorkDir",
            "LaunchSubScript",
            "AbortSubScript",
            "IsSubScriptRunning",
            "ConPrintTable",
            "ConExecute",
            "ConClear",
            "SpawnProcess",
            "OpenURL",
            "SetProfiling",
            "Restart",
            "Exit",
            "TakeScreenshot",
            "NewFileSearch",
        ] {
            set(name, func(name, none));
        }
        set(
            "GetScreenSize",
            func("GetScreenSize", |_| Ok(vec![n(1920.0), n(1080.0)])),
        );
        set(
            "GetVirtualScreenSize",
            func("GetVirtualScreenSize", |_| Ok(vec![n(1920.0), n(1080.0)])),
        );
        set(
            "GetScreenScale",
            func("GetScreenScale", |_| Ok(vec![n(1.0)])),
        );
        set(
            "GetDPIScaleOverridePercent",
            func("GetDPIScaleOverridePercent", |_| Ok(vec![n(1.0)])),
        );
        set(
            "DrawStringWidth",
            func("DrawStringWidth", |_| Ok(vec![n(1.0)])),
        );
        set(
            "DrawStringCursorIndex",
            func("DrawStringCursorIndex", |_| Ok(vec![n(0.0)])),
        );
        set(
            "StripEscapes",
            func("StripEscapes", |a| {
                let t = first(rt::method(
                    &arg(a, 0),
                    &LuaStr::from("gsub"),
                    &[s("%^%d"), s("")],
                )?);
                Ok(vec![first(rt::method(
                    &t,
                    &LuaStr::from("gsub"),
                    &[s("%^x%x%x%x%x%x%x"), s("")],
                )?)])
            }),
        );
        set("GetAsyncCount", func("GetAsyncCount", |_| Ok(vec![n(0.0)])));
        set(
            "GetCursorPos",
            func("GetCursorPos", |_| Ok(vec![n(0.0), n(0.0)])),
        );
        // TODO in upstream too: build codes cannot be loaded headless
        set("Deflate", func("Deflate", |_| Ok(vec![s("")])));
        set("Inflate", func("Inflate", |_| Ok(vec![s("")])));
        set("GetTime", func("GetTime", |_| Ok(vec![n(0.0)])));
        for name in [
            "GetScriptPath",
            "GetRuntimePath",
            "GetUserPath",
            "GetWorkDir",
        ] {
            set(name, func(name, |_| Ok(vec![s("")])));
        }
        set(
            "GetCloudProvider",
            func("GetCloudProvider", |_| {
                Ok(vec![Value::Nil, Value::Nil, Value::Nil])
            }),
        );
        set(
            "ConPrintf",
            func("ConPrintf", |a| {
                let msg = lua::env::string_format(a)?;
                let _ = msg;
                Ok(vec![])
            }),
        );

        // Files: read only
        let files = data.clone();
        if let Value::Table(io) = env.global("io") {
            io.set_str(
                "open",
                func("io.open", move |a| {
                    let path = lua::to_string(&arg(a, 0)).to_string();
                    let mode = arg(a, 1);
                    let mode = if mode.is_nil() {
                        "r".to_string()
                    } else {
                        lua::to_string(&mode).to_string()
                    };
                    if !mode.starts_with('r') || mode.contains('+') {
                        return Ok(vec![
                            Value::Nil,
                            s(&format!("{path}: writing is not supported")),
                        ]);
                    }
                    match files.read_file(&path) {
                        Some(bytes) => Ok(vec![file_handle(bytes)]),
                        None => Ok(vec![
                            Value::Nil,
                            s(&format!("{path}: No such file or directory")),
                        ]),
                    }
                }),
            );
        }

        // loadstring, which upstream uses on data only (the passive tree)
        let ls_env = Rc::downgrade(env);
        set(
            "loadstring",
            func("loadstring", move |a| {
                let text = arg(a, 0);
                let Value::Str(text) = text else {
                    return err("bad argument #1 to 'loadstring' (string expected)");
                };
                if let Err(e) = lua::parser::parse(text.as_bytes()) {
                    return Ok(vec![
                        Value::Nil,
                        s(&format!("[string]:{}: {}", e.line, e.msg)),
                    ]);
                }
                let env = ls_env.clone();
                Ok(vec![func("loadstring chunk", move |args| {
                    let env = env
                        .upgrade()
                        .ok_or_else(|| lua::LuaError::msg("environment dropped"))?;
                    crate::imported::evaluate(&env, "loadstring", text.as_bytes(), args)
                })])
            }),
        );

        // Modules
        let req_data = data.clone();
        let load_env = Rc::downgrade(env);
        let load = func("LoadModule", move |a| {
            let env = load_env
                .upgrade()
                .ok_or_else(|| lua::LuaError::msg("environment dropped"))?;
            let name = lua::to_string(&arg(a, 0)).to_string();
            let args = a.get(1..).unwrap_or(&[]);
            load_module(&env, &*data, &name, args)
        });
        env.set("LoadModule", load.clone());
        let pload = load.clone();
        set(
            "PLoadModule",
            func("PLoadModule", move |a| match rt::call(&pload, a) {
                Ok(mut r) => {
                    r.insert(0, Value::Nil);
                    Ok(r)
                }
                Err(e) => {
                    trace(&e);
                    Ok(vec![e.into_value()])
                }
            }),
        );
        set(
            "PCall",
            func("PCall", |a| {
                match rt::call(&arg(a, 0), a.get(1..).unwrap_or(&[])) {
                    Ok(mut r) => {
                        r.insert(0, Value::Nil);
                        Ok(r)
                    }
                    Err(e) => {
                        trace(&e);
                        Ok(vec![e.into_value()])
                    }
                }
            }),
        );

        // jit.opt.start, used by Launch.lua
        if let Value::Table(jit) = env.global("jit") {
            let opt = TableRef::new();
            opt.set_str("start", func("jit.opt.start", none));
            jit.set_str("opt", Value::Table(opt));
        }

        // require: translated modules and runtime libraries, data, natives,
        // or nothing; upstream runs with src/ as the working directory
        *env.loader.borrow_mut() = Some(Box::new(
            move |env: &Rc<Env>, name: &str| -> LResult<Value> {
                match name {
                    // HeadlessWrapper's hack: no lcurl
                    "lcurl.safe" | "lua-profiler" | "socket" => Ok(Value::Nil),
                    "lua-utf8" => Ok(crate::utf8::library()),
                    "jit" => Ok(env.global("jit")),
                    "sha1" => Ok(Value::Table(TableRef::new())),
                    _ => {
                        let rel = name.replace('.', "/");
                        if let Some(chunk) = crate::up::chunk(&format!("src/{rel}.lua")) {
                            return Ok(first(
                                chunk(env, &[s(name)]).map_err(|e| in_chunk(e, &rel))?,
                            ));
                        }
                        if rel.starts_with("Data/") || rel.starts_with("TreeData/") {
                            return Ok(first(req_data.load(env, &rel, &[s(name)])?));
                        }
                        let path = format!("runtime/lua/{name}.lua");
                        match crate::up::chunk(&path) {
                            Some(chunk) => Ok(first(
                                chunk(env, &[s(name)]).map_err(|e| in_chunk(e, &path))?,
                            )),
                            None => err(format!("module '{name}' not found")),
                        }
                    }
                }
            },
        ));
    }

    /// Starts the program as HeadlessWrapper does: Launch.lua, `OnInit` and
    /// one frame.
    pub fn start(self: &Rc<Self>) -> LResult<()> {
        // As oracle/headless.lua: no command line arguments
        self.env.set("arg", Value::Table(TableRef::new()));
        let launch = crate::up::chunk("src/Launch.lua").expect("Launch.lua is translated");
        launch(&self.env, &[])?;
        self.run_callback("OnInit", &[])?;
        self.run_callback("OnFrame", &[])?;
        self.check_prompt()
    }

    /// Errors upstream caught show up as a prompt; fail with it instead.
    fn check_prompt(&self) -> LResult<()> {
        let main = self.main_object.borrow().clone();
        let prompt = rt::index(&main, &s("promptMsg"))?;
        if prompt.truthy() {
            return err(format!("upstream error: {}", lua::to_string(&prompt)));
        }
        Ok(())
    }

    /// `mainObject.main`
    pub fn main(&self) -> LResult<Value> {
        let main = self.main_object.borrow().clone();
        rt::index(&main, &s("main"))
    }

    /// The build mode: `mainObject.main.modes["BUILD"]`.
    pub fn build(&self) -> LResult<Value> {
        rt::index(&rt::index(&self.main()?, &s("modes"))?, &s("BUILD"))
    }

    /// HeadlessWrapper's `newBuild()`.
    pub fn new_build(&self) -> LResult<()> {
        let main = self.main()?;
        rt::method(
            &main,
            &LuaStr::from("SetMode"),
            &[s("BUILD"), Value::Bool(false), s("Unnamed build")],
        )?;
        self.run_callback("OnFrame", &[])?;
        Ok(())
    }

    /// Imports a PoE API character JSON as `oracle/headless.lua` does: the
    /// import calls of upstream's TestImport_spec.
    pub fn load_build_json(&self, json: &[u8]) -> LResult<()> {
        let dkjson = rt::call(&self.global("require"), &[s("dkjson")])?;
        let decode = rt::index(&first(dkjson), &s("decode"))?;
        let data = first(rt::call(&decode, &[Value::Str(json.into())])?);
        let ch = rt::index(&data, &s("character"))?;
        let data = if ch.truthy() { ch } else { data };
        self.new_build()?;
        let tab = rt::index(&self.build()?, &s("importTab"))?;
        rt::method(
            &tab,
            &LuaStr::from("ImportPassiveTreeAndJewels"),
            &[data.clone(), Value::Bool(true)],
        )?;
        rt::method(&tab, &LuaStr::from("ImportItemsAndSkills"), &[data])?;
        self.run_callback("OnFrame", &[])?;
        self.check_prompt()
    }

    /// HeadlessWrapper's `loadBuildFromXML(xmlText, name)`.
    pub fn load_build_xml(&self, xml: &[u8], name: &str) -> LResult<()> {
        let main = self.main()?;
        rt::method(
            &main,
            &LuaStr::from("SetMode"),
            &[
                s("BUILD"),
                Value::Bool(false),
                s(name),
                Value::Str(xml.into()),
            ],
        )?;
        self.run_callback("OnFrame", &[])?;
        self.check_prompt()
    }
}

/// Names a module's top level in an error's traceback.
fn in_chunk(mut e: lua::LuaError, path: &str) -> lua::LuaError {
    e.push_frame(format!("{path} (top level)").into());
    e
}

/// With `UPSTREAM_TRACE` set, errors upstream catches are printed with
/// their traceback.
fn trace(e: &lua::LuaError) {
    if std::env::var_os("UPSTREAM_TRACE").is_some() {
        eprintln!(
            "upstream error: {}\n  {}",
            lua::to_string(e.value()),
            e.traceback().join("\n  ")
        );
    }
}

/// A file opened for reading: `read("*a")`, `read("*l")`, `lines()`, `close()`.
fn file_handle(bytes: Vec<u8>) -> Value {
    let pos = Rc::new(RefCell::new(0usize));
    let bytes = Rc::new(bytes);
    let h = TableRef::new();
    let read_line = {
        let (bytes, pos) = (bytes.clone(), pos.clone());
        move || -> Value {
            let mut p = pos.borrow_mut();
            if *p >= bytes.len() {
                return Value::Nil;
            }
            let end = bytes[*p..]
                .iter()
                .position(|&b| b == b'\n')
                .map(|i| *p + i)
                .unwrap_or(bytes.len());
            let line = Value::Str(bytes[*p..end].into());
            *p = end + 1;
            line
        }
    };
    let rl = read_line.clone();
    let (b2, p2) = (bytes.clone(), pos.clone());
    h.set_str(
        "read",
        func("file:read", move |a| {
            let fmt = arg(a, 1);
            let fmt = if fmt.is_nil() {
                "*l".to_string()
            } else {
                lua::to_string(&fmt).to_string()
            };
            if fmt.starts_with("*a") {
                let mut p = p2.borrow_mut();
                let v = Value::Str(b2[(*p).min(b2.len())..].into());
                *p = b2.len();
                Ok(vec![v])
            } else if fmt.starts_with("*l") {
                Ok(vec![rl()])
            } else {
                err(format!("file:read format {fmt} is not supported"))
            }
        }),
    );
    h.set_str(
        "lines",
        func("file:lines", move |_| {
            Ok(vec![func("lines iterator", {
                let rl = read_line.clone();
                move |_| Ok(vec![rl()])
            })])
        }),
    );
    h.set_str("close", func("file:close", |_| Ok(vec![Value::Bool(true)])));
    Value::Table(h)
}

/// `LoadModule(name, ...)`: translated modules run, data files come from the
/// data source.
fn load_module(
    env: &Rc<Env>,
    data: &dyn DataSource,
    name: &str,
    args: &[Value],
) -> LResult<Vec<Value>> {
    let name = name.strip_suffix(".lua").unwrap_or(name);
    if name.starts_with("Data/") || name.starts_with("TreeData/") {
        return data.load(env, name, args);
    }
    match crate::up::chunk(&module_path(name)) {
        Some(chunk) => chunk(env, args).map_err(|e| in_chunk(e, &module_path(name))),
        None => err(format!(
            "LoadModule() error loading '{name}.lua': not translated"
        )),
    }
}
