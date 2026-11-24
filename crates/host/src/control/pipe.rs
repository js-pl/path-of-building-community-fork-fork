//! The control socket on Windows: a named pipe only the user can open.
//!
//! The pipe's security descriptor grants the current user, and no one
//! else, full access (`D:P(A;;GA;;;<user's SID>)`, the equivalent of
//! 0600), and remote clients are rejected. The host makes the first
//! instance with `FILE_FLAG_FIRST_PIPE_INSTANCE`, so it never joins a pipe
//! someone else made; a client checks that the pipe's owner is the current
//! user before it says hello, so it never talks to a pipe someone else
//! squatted on.

use super::*;
use std::fs::File;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, RawHandle};
use std::path::Path;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_NO_DATA, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED,
    GetLastError, HANDLE, INVALID_HANDLE_VALUE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
    SDDL_REVISION_1, SE_KERNEL_OBJECT,
};
use windows_sys::Win32::Security::{
    EqualSid, GetTokenInformation, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX, SECURITY_IDENTIFICATION,
};
use windows_sys::Win32::System::IO::CancelIoEx;
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    WaitNamedPipeW,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// Pipe names start with this
pub const PREFIX: &str = r"\\.\pipe\";

/// The default pipe: the app's name and the user's, so users don't share
/// one (each one's is theirs alone anyway)
pub fn default_name() -> String {
    let user: String = std::env::var("USERNAME")
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect();
    format!("{PREFIX}path-of-building-fork-fork-{user}-control")
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn last_error(what: &str) -> String {
    format!("{what}: {}", std::io::Error::last_os_error())
}

fn name_of(path: &Path) -> Result<String, String> {
    let name = path.to_string_lossy().to_string();
    if !name.starts_with(PREFIX) || name.len() == PREFIX.len() {
        return Err(format!(
            "{name}: the control socket on Windows is a named pipe, {PREFIX}<name>"
        ));
    }
    Ok(name)
}

/// The current user's SID, as the token gives it
struct UserSid {
    buf: Vec<u64>,
}

impl UserSid {
    fn current() -> Result<UserSid, String> {
        unsafe {
            let mut token: HANDLE = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(last_error("the process's token"));
            }
            let mut len = 0u32;
            GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut len);
            let mut buf = vec![0u64; (len as usize).div_ceil(8).max(1)];
            let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len);
            CloseHandle(token);
            if ok == 0 {
                return Err(last_error("the token's user"));
            }
            Ok(UserSid { buf })
        }
    }

    fn sid(&self) -> PSID {
        unsafe { (*self.buf.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    }

    fn string(&self) -> Result<String, String> {
        unsafe {
            let mut s: *mut u16 = std::ptr::null_mut();
            if ConvertSidToStringSidW(self.sid(), &mut s) == 0 {
                return Err(last_error("the user's SID"));
            }
            let len = (0..).take_while(|&i| *s.add(i) != 0).count();
            let out = String::from_utf16_lossy(std::slice::from_raw_parts(s, len));
            LocalFree(s.cast());
            Ok(out)
        }
    }
}

/// The pipe's security descriptor: owned by the user, who alone has access
struct Descriptor(PSECURITY_DESCRIPTOR);

// The descriptor is only read once made
unsafe impl Send for Descriptor {}

impl Descriptor {
    fn user_only() -> Result<Descriptor, String> {
        let sid = UserSid::current()?.string()?;
        let sddl = wide(&format!("O:{sid}D:P(A;;GA;;;{sid})"));
        let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut sd,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(last_error("the pipe's security descriptor"));
        }
        Ok(Descriptor(sd))
    }
}

impl Drop for Descriptor {
    fn drop(&mut self) {
        unsafe { LocalFree(self.0) };
    }
}

/// A named pipe the host listens on: the instance waiting for the next
/// client, and how to make more
pub struct Listener {
    name: Vec<u16>,
    sd: Descriptor,
    next: File,
}

impl Listener {
    fn instance(name: &[u16], sd: &Descriptor, first: bool) -> std::io::Result<File> {
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.0,
            bInheritHandle: 0,
        };
        let h = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX
                    | if first {
                        FILE_FLAG_FIRST_PIPE_INSTANCE
                    } else {
                        0
                    },
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                64 * 1024,
                64 * 1024,
                0,
                &sa,
            )
        };
        if h == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error());
        }
        Ok(unsafe { File::from_raw_handle(h as RawHandle) })
    }

    /// Waits for the next client: its end of the pipe
    pub fn accept(&mut self) -> std::io::Result<File> {
        loop {
            let h = self.next.as_raw_handle() as HANDLE;
            if unsafe { ConnectNamedPipe(h, std::ptr::null_mut()) } != 0 {
                break;
            }
            match unsafe { GetLastError() } {
                ERROR_PIPE_CONNECTED => break,
                // A client came and went before it was accepted (a probe
                // whether a host runs): the instance is reused
                ERROR_NO_DATA => {
                    unsafe { DisconnectNamedPipe(h) };
                }
                // Anything else: a fresh instance, so one broken instance
                // never stops the host from answering
                e => {
                    self.next = Self::instance(&self.name, &self.sd, false)?;
                    return Err(std::io::Error::from_raw_os_error(e as i32));
                }
            }
        }
        // The next instance before this one is handed out, so the pipe
        // never stops existing
        let next = Self::instance(&self.name, &self.sd, false)?;
        Ok(std::mem::replace(&mut self.next, next))
    }
}

/// Listens on the pipe `path` (`\\.\pipe\<name>`). A pipe that exists
/// already is a running host's, or someone else's: an error either way.
pub fn listen(path: &Path) -> Result<Listener, String> {
    let name = name_of(path)?;
    let sd = Descriptor::user_only()?;
    let wname = wide(&name);
    let next = Listener::instance(&wname, &sd, true).map_err(|e| {
        if e.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32) {
            if answers(path) {
                format!("a host is running already on {name}")
            } else {
                format!("{name} exists and isn't this user's host's")
            }
        } else {
            format!("{name}: {e}")
        }
    })?;
    Ok(Listener {
        name: wname,
        sd,
        next,
    })
}

/// How to close a connection from another thread: its pending read is
/// cancelled and the client disconnected
fn closer(f: &File) -> Closer {
    let h = f.as_raw_handle() as usize;
    Box::new(move || unsafe {
        DisconnectNamedPipe(h as HANDLE);
        CancelIoEx(h as HANDLE, std::ptr::null());
    })
}

/// Accepts connections on a thread of their own each, passing their
/// calls to `calls` and listing them in `registry`
pub fn accept(
    mut listener: Listener,
    calls: mpsc::Sender<Call>,
    registry: std::sync::Arc<Registry>,
) {
    loop {
        let stream = match listener.accept() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("pob host: the control pipe: {e}");
                std::thread::sleep(std::time::Duration::from_millis(100));
                continue;
            }
        };
        let calls = calls.clone();
        let registry = registry.clone();
        std::thread::spawn(move || {
            let close = closer(&stream);
            let _ = connection_in(&stream, calls, now(), Some((&registry, close)));
        });
    }
}

/// Opens the pipe `path` as a client, after checking that its owner is
/// the current user
fn open(path: &Path) -> Result<File, String> {
    let name = name_of(path)?;
    let mut waited = false;
    let f = loop {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .security_qos_flags(SECURITY_IDENTIFICATION)
            .open(&name)
        {
            Ok(f) => break f,
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) && !waited => {
                waited = true;
                unsafe { WaitNamedPipeW(wide(&name).as_ptr(), 5000) };
            }
            Err(e) => return Err(format!("no host on {name}: {e}")),
        }
    };
    let me = UserSid::current()?;
    let mut owner: PSID = std::ptr::null_mut();
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    let e = unsafe {
        GetSecurityInfo(
            f.as_raw_handle() as HANDLE,
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut sd,
        )
    };
    if e != 0 {
        return Err(format!(
            "{name}: its owner: {}",
            std::io::Error::from_raw_os_error(e as i32)
        ));
    }
    let mine = unsafe { EqualSid(owner, me.sid()) } != 0;
    unsafe { LocalFree(sd) };
    if !mine {
        return Err(format!("{name} isn't this user's: not connecting"));
    }
    Ok(f)
}

/// Whether a host answers on `path`
pub fn answers(path: &Path) -> bool {
    open(path).is_ok()
}

/// An `admin` request to the host on `path`
pub fn admin(path: &Path, req: J) -> Result<J, String> {
    let f = open(path)?;
    let mut w = &f;
    writeln!(w, "{}", json!({ "admin": req })).map_err(|e| e.to_string())?;
    let line = read_line(&mut BufReader::new(&f))
        .map_err(|e| e.to_string())?
        .ok_or("the host closed the connection")?;
    let v: J = serde_json::from_str(&line).map_err(|e| e.to_string())?;
    match v.get("error") {
        Some(e) => Err(e.as_str().unwrap_or_default().to_string()),
        None => Ok(v["ok"].clone()),
    }
}

/// A client of the control pipe
pub struct Client {
    reader: BufReader<File>,
    writer: File,
    /// The hello's answer
    pub hello: J,
}

impl Client {
    /// Connects and says hello
    pub fn connect(path: &Path, hello: &J) -> Result<Client, String> {
        let f = open(path).map_err(|e| format!("{e} (start one with `pob host --headless`)"))?;
        let writer = f.try_clone().map_err(|e| e.to_string())?;
        let mut c = Client {
            reader: BufReader::new(f),
            writer,
            hello: J::Null,
        };
        let h = c.call(hello)?;
        if let Some(e) = h.get("error") {
            return Err(e.as_str().unwrap_or_default().to_string());
        }
        c.hello = h["ok"].clone();
        Ok(c)
    }

    /// One request's answer, `{"ok": ...}` or `{"error": ...}`
    pub fn call(&mut self, req: &J) -> Result<J, String> {
        writeln!(self.writer, "{req}").map_err(|e| e.to_string())?;
        let line = read_line(&mut self.reader)
            .map_err(|e| e.to_string())?
            .ok_or("the host closed the connection")?;
        serde_json::from_str(&line).map_err(|e| e.to_string())
    }
}
