//! Runs a test's work in child processes of the test binary, in parallel.
//!
//! The translated program keeps about 12 MB of every build it loads, next
//! to the 1.6 GB it starts with, so tests that load the corpus many times
//! split their builds into shards, each run by a fresh process. The
//! children run at once as far as the machine allows: one per core, and as
//! many as fit in its memory at `TEST_SLOT_GB` each (default 3). The limit
//! holds over every test process on the machine (`cargo test` runs a
//! binary's tests on threads, nextest each test in a process of its own),
//! through lock files in the temporary directory. `TEST_JOBS=<n>` sets the
//! number of children at once. `TEST_SHARD_PARTITION=<k>/<n>` runs only
//! the `k`-th of every `n` shards (from 1), to split a test over machines.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

/// The environment variable that gives a child its shard
const SHARD: &str = "TESTKIT_SHARD";

/// The shard this process runs, when it is a child of `run_shards`
pub fn shard() -> Option<String> {
    std::env::var(SHARD).ok()
}

/// Children that may run at once on this machine
pub fn jobs() -> usize {
    if let Some(n) = std::env::var("TEST_JOBS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
    {
        return n.max(1);
    }
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let slot_gb: f64 = std::env::var("TEST_SLOT_GB")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3.0);
    // The memory of the machine, not what is free now: every process
    // computes the same count
    let total_gb = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|m| {
            let line = m.lines().find(|l| l.starts_with("MemTotal:"))?;
            let kb: f64 = line.split_whitespace().nth(1)?.parse().ok()?;
            Some(kb / 1024.0 / 1024.0)
        })
        .unwrap_or(8.0);
    // A gigabyte for the parents and the system
    let by_memory = ((total_gb - 1.0) / slot_gb).floor() as usize;
    cores.min(by_memory).max(1)
}

/// One of the machine's `jobs()` slots, held until dropped
pub struct Slot(#[allow(dead_code)] File);

/// Waits for a free slot
pub fn slot() -> Slot {
    let n = jobs();
    let dir = std::env::temp_dir().join("testkit-slots");
    std::fs::create_dir_all(&dir).unwrap();
    loop {
        for k in 0..n {
            let path: PathBuf = dir.join(format!("slot-{k}"));
            let file = OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&path)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            match file.try_lock() {
                Ok(()) => return Slot(file),
                Err(TryLockError::WouldBlock) => {}
                Err(TryLockError::Error(e)) => panic!("{}: {e}", path.display()),
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Runs each of `shards` in a child process: this test binary again, with
/// only the test `test` and the shard in `TESTKIT_SHARD`, at most `jobs()`
/// children at once over the machine. The children's stderr lines that
/// contain any of `report` are printed; panics with the output of every
/// child that failed.
pub fn run_shards(test: &str, shards: &[String], report: &[&str]) {
    let shards: Vec<String> = match std::env::var("TEST_SHARD_PARTITION") {
        Ok(p) if !p.is_empty() => {
            let (k, n) = p
                .split_once('/')
                .and_then(|(k, n)| Some((k.parse::<usize>().ok()?, n.parse::<usize>().ok()?)))
                .filter(|&(k, n)| k >= 1 && k <= n)
                .unwrap_or_else(|| panic!("TEST_SHARD_PARTITION={p}: not <k>/<n>"));
            shards
                .iter()
                .enumerate()
                .filter(|(i, _)| i % n == k - 1)
                .map(|(_, s)| s.clone())
                .collect()
        }
        _ => shards.to_vec(),
    };
    let shards = &shards[..];
    let exe = std::env::current_exe().unwrap();
    let start = Instant::now();
    let outputs: Vec<(String, Output, Duration)> = std::thread::scope(|scope| {
        let runs: Vec<_> = shards
            .iter()
            .map(|shard| {
                let exe = &exe;
                scope.spawn(move || {
                    let slot = slot();
                    let t = Instant::now();
                    let out = Command::new(exe)
                        .args([test, "--exact", "--nocapture", "--test-threads=1"])
                        .env(SHARD, shard)
                        .output()
                        .unwrap_or_else(|e| panic!("{}: {e}", exe.display()));
                    drop(slot);
                    (shard.clone(), out, t.elapsed())
                })
            })
            .collect();
        runs.into_iter().map(|r| r.join().unwrap()).collect()
    });
    let mut failures = Vec::new();
    for (shard, out, time) in &outputs {
        let text = String::from_utf8_lossy(&out.stderr);
        if out.status.success() {
            eprintln!("{test} [{shard}]: {time:.1?}");
            for line in text
                .lines()
                .filter(|l| report.iter().any(|r| l.contains(r)))
            {
                eprintln!("  {line}");
            }
        } else {
            let stdout = String::from_utf8_lossy(&out.stdout);
            failures.push(format!("[{shard}] {}:\n{stdout}\n{text}", out.status));
        }
    }
    eprintln!(
        "{test}: {} shards in {:.1?}, {} at once",
        shards.len(),
        start.elapsed(),
        jobs()
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
