//! Independent calculations on threads: natively on a `rayon` pool, in
//! the browser one after the other. Results keep the order of their
//! inputs, so what is made of them does not depend on the thread count.

/// Where calculations run
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Threads {
    /// One after the other, on this thread
    Sequential,
    /// On the pool this thread works in, or rayon's global pool
    Current,
    /// On a pool of this many threads of their own
    Pool(usize),
}

impl Threads {
    /// `Pool(n)` for `Some(n)`, else `Sequential`
    pub fn from_count(threads: Option<usize>) -> Threads {
        threads.map_or(Threads::Sequential, Threads::Pool)
    }

    /// As many threads as the machine has natively, one in the browser
    pub fn available() -> Threads {
        #[cfg(not(target_arch = "wasm32"))]
        {
            Threads::Pool(std::thread::available_parallelism().map_or(1, |n| n.get()))
        }
        #[cfg(target_arch = "wasm32")]
        {
            Threads::Sequential
        }
    }
}

thread_local! {
    static FULL_DPS_THREADS: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

/// Whether the Full DPS skills of a calculation on this thread are
/// calculated on threads ([`Threads::Current`]); both give the same
/// results. Never in the browser.
pub fn full_dps_threads() -> bool {
    !cfg!(target_arch = "wasm32") && FULL_DPS_THREADS.with(|t| t.get())
}

/// Sets [`full_dps_threads`] on this thread, for comparing the two
pub fn set_full_dps_threads(on: bool) {
    FULL_DPS_THREADS.with(|t| t.set(on));
}

/// `f` of each item, in the items' order. The calculations' thread-local
/// settings ([`crate::calc_env::reuse_setup`], [`full_dps_threads`]) carry
/// over to the threads.
#[cfg(not(target_arch = "wasm32"))]
pub fn map<T, R, F>(items: Vec<T>, threads: Threads, f: F) -> Vec<R>
where
    T: Send,
    R: Send,
    F: Fn(T) -> R + Sync + Send,
{
    use rayon::prelude::*;
    if threads == Threads::Sequential || items.len() < 2 {
        return items.into_iter().map(f).collect();
    }
    let reuse = crate::calc_env::reuse_setup();
    let full_dps = full_dps_threads();
    let run = || {
        items
            .into_par_iter()
            .map(|x| {
                crate::calc_env::set_reuse_setup(reuse);
                set_full_dps_threads(full_dps);
                f(x)
            })
            .collect()
    };
    match threads {
        Threads::Pool(n) => match rayon::ThreadPoolBuilder::new().num_threads(n).build() {
            Ok(pool) => pool.install(run),
            Err(_) => run(),
        },
        _ => run(),
    }
}

#[cfg(target_arch = "wasm32")]
pub fn map<T, R, F>(items: Vec<T>, _threads: Threads, f: F) -> Vec<R>
where
    F: Fn(T) -> R,
{
    items.into_iter().map(f).collect()
}
