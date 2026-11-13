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

/// `f` of each item, in the items' order. The calculations' thread-local
/// setting ([`crate::calc_env::reuse_setup`]) carries over to the threads.
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
    let run = || {
        items
            .into_par_iter()
            .map(|x| {
                crate::calc_env::set_reuse_setup(reuse);
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
