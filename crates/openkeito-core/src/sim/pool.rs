//! The threads that solve the parts of a divided circuit side by side.
//!
//! A time step takes microseconds, so the threads must take up their tasks
//! within a fraction of that: they wait for the next step in a loop (and
//! only go to sleep when none comes for a while), and a step hands them its
//! tasks by storing one number. The thread that runs the time loop takes
//! tasks as well, and goes on when all are done.
//!
//! The tasks of one call are independent of each other (the solver gives
//! each one memory that no other task touches), so which thread takes which
//! task, and how many threads there are, does not change any result.
//!
//! Without threads (WebAssembly) the tasks run one after the other.

#[cfg(not(target_arch = "wasm32"))]
mod threads {
    use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicPtr, AtomicUsize, Ordering::SeqCst};
    use std::sync::{Arc, Condvar, Mutex};
    use std::thread::JoinHandle;
    use std::time::Duration;

    /// The tasks of one call: task `i` is `job(i)`.
    type Job<'a> = &'a (dyn Fn(usize) + Sync);

    /// Rounds of the waiting loop before a thread goes to sleep: some
    /// milliseconds (a round is one pause instruction, 10 to 40 ns), far
    /// more than the time between two steps of a run that uses the threads.
    const SPIN_ROUNDS: u32 = 200_000;

    struct Shared {
        /// Points to the `Job` of the call being run. Read only by a thread
        /// that has claimed a task, which the call waits for.
        job: AtomicPtr<()>,
        /// Tasks not yet claimed: a thread that takes it from n > 0 to n - 1
        /// has claimed task n - 1.
        unclaimed: AtomicIsize,
        /// Tasks claimed or unclaimed that have not finished.
        pending: AtomicUsize,
        /// A task panicked.
        panicked: AtomicBool,
        stop: AtomicBool,
        /// Threads that sleep until they are woken.
        sleeping: AtomicUsize,
        lock: Mutex<()>,
        wake: Condvar,
    }

    impl Shared {
        /// Claims and runs tasks until none is left.
        fn work(&self) {
            while self.unclaimed.load(SeqCst) > 0 {
                let left = self.unclaimed.fetch_sub(1, SeqCst);
                if left <= 0 {
                    break;
                }
                // SAFETY: `job` was stored before `unclaimed` became
                // positive, and the call that stored it does not return
                // before `pending` is 0, which this task holds above 0 until
                // it has finished: the `Job` it points to is alive.
                let job: Job = unsafe { *(self.job.load(SeqCst) as *const Job) };
                let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    job((left - 1) as usize)
                }));
                if done.is_err() {
                    self.panicked.store(true, SeqCst);
                }
                self.pending.fetch_sub(1, SeqCst);
            }
        }

        /// The loop of a thread of the pool.
        fn serve(&self) {
            let mut idle = 0;
            while !self.stop.load(SeqCst) {
                if self.unclaimed.load(SeqCst) > 0 {
                    self.work();
                    idle = 0;
                } else if idle < SPIN_ROUNDS {
                    idle += 1;
                    std::hint::spin_loop();
                } else {
                    let guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
                    self.sleeping.fetch_add(1, SeqCst);
                    // A call that stored its tasks before this point is seen
                    // here; one that stores them later sees `sleeping`.
                    if self.unclaimed.load(SeqCst) <= 0 && !self.stop.load(SeqCst) {
                        let _ = self
                            .wake
                            .wait_timeout(guard, Duration::from_millis(50))
                            .unwrap_or_else(|e| e.into_inner());
                    }
                    self.sleeping.fetch_sub(1, SeqCst);
                    idle = 0;
                }
            }
        }

        fn wake_sleepers(&self) {
            if self.sleeping.load(SeqCst) > 0 {
                let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
                self.wake.notify_all();
            }
        }
    }

    /// Threads that run the tasks of `run` together with the calling thread.
    pub struct Pool {
        shared: Arc<Shared>,
        workers: Vec<JoinHandle<()>>,
    }

    impl Pool {
        /// A pool of `threads` threads, the calling one included. `None`
        /// when that is one thread, or when no thread could be started.
        pub fn new(threads: usize) -> Option<Pool> {
            if threads < 2 {
                return None;
            }
            let shared = Arc::new(Shared {
                job: AtomicPtr::new(std::ptr::null_mut()),
                unclaimed: AtomicIsize::new(0),
                pending: AtomicUsize::new(0),
                panicked: AtomicBool::new(false),
                stop: AtomicBool::new(false),
                sleeping: AtomicUsize::new(0),
                lock: Mutex::new(()),
                wake: Condvar::new(),
            });
            let mut workers = Vec::with_capacity(threads - 1);
            for k in 1..threads {
                let shared = shared.clone();
                let spawned = std::thread::Builder::new()
                    .name(format!("openkeito-part-{k}"))
                    .spawn(move || shared.serve());
                match spawned {
                    Ok(handle) => workers.push(handle),
                    Err(_) => break,
                }
            }
            if workers.is_empty() {
                return None;
            }
            Some(Pool { shared, workers })
        }

        /// Number of threads, the calling one included.
        pub fn threads(&self) -> usize {
            self.workers.len() + 1
        }

        /// Runs `job(0)` .. `job(tasks - 1)`, each once, on the threads of
        /// the pool and the calling thread, and returns when all are done.
        pub fn run(&self, tasks: usize, job: Job) {
            if tasks == 0 {
                return;
            }
            let s = &*self.shared;
            // No task of an earlier call is unclaimed or running: nothing
            // reads `job` now.
            s.job.store(&job as *const Job as *mut (), SeqCst);
            s.pending.store(tasks, SeqCst);
            s.unclaimed.store(tasks as isize, SeqCst);
            s.wake_sleepers();
            s.work();
            while s.pending.load(SeqCst) > 0 {
                std::hint::spin_loop();
            }
            // Threads that looked for a task after the last one was claimed
            // left `unclaimed` below 0; the next call stores it anew.
            if s.panicked.swap(false, SeqCst) {
                panic!("a part of the circuit panicked while it was solved");
            }
        }
    }

    impl Drop for Pool {
        fn drop(&mut self) {
            self.shared.stop.store(true, SeqCst);
            {
                let _guard = self.shared.lock.lock().unwrap_or_else(|e| e.into_inner());
                self.shared.wake.notify_all();
            }
            for worker in self.workers.drain(..) {
                let _ = worker.join();
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) use threads::Pool;

/// Without threads there is no pool.
#[cfg(target_arch = "wasm32")]
pub(super) struct Pool;

#[cfg(target_arch = "wasm32")]
impl Pool {
    pub fn new(_threads: usize) -> Option<Pool> {
        None
    }

    pub fn threads(&self) -> usize {
        1
    }

    pub fn run(&self, tasks: usize, job: &(dyn Fn(usize) + Sync)) {
        (0..tasks).for_each(job);
    }
}

/// Number of threads the machine offers to this process.
pub(super) fn available_threads() -> usize {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::thread::available_parallelism().map_or(1, |n| n.get())
    }
    #[cfg(target_arch = "wasm32")]
    {
        1
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    use super::Pool;

    #[test]
    fn every_task_runs_once_whatever_the_number_of_threads() {
        for threads in [2, 3, 8] {
            let pool = Pool::new(threads).expect("threads start");
            assert_eq!(pool.threads(), threads);
            for tasks in [1, 2, 7, 64] {
                // Each task has memory of its own, as the parts of a
                // circuit have.
                let cells: Vec<Mutex<u64>> = (0..tasks).map(|_| Mutex::new(0)).collect();
                let calls = AtomicU64::new(0);
                for round in 0..200 {
                    pool.run(tasks, &|i| {
                        *cells[i].lock().unwrap() += i as u64 + round;
                        calls.fetch_add(1, Ordering::Relaxed);
                    });
                }
                assert_eq!(calls.load(Ordering::Relaxed), 200 * tasks as u64);
                for (i, cell) in cells.iter().enumerate() {
                    let expected: u64 = (0..200).map(|round| i as u64 + round).sum();
                    assert_eq!(*cell.lock().unwrap(), expected);
                }
            }
        }
    }

    #[test]
    fn threads_that_went_to_sleep_take_up_tasks_again() {
        let pool = Pool::new(3).expect("threads start");
        let calls = AtomicU64::new(0);
        pool.run(5, &|_| {
            calls.fetch_add(1, Ordering::Relaxed);
        });
        // Longer than the waiting loop.
        std::thread::sleep(std::time::Duration::from_millis(300));
        pool.run(5, &|_| {
            calls.fetch_add(1, Ordering::Relaxed);
        });
        assert_eq!(calls.load(Ordering::Relaxed), 10);
    }

    #[test]
    fn a_task_that_panics_ends_the_call_with_a_panic() {
        let pool = Pool::new(2).expect("threads start");
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            pool.run(4, &|i| assert!(i != 2, "task 2 fails"));
        }));
        assert!(result.is_err());
        // The pool is still usable.
        let calls = AtomicU64::new(0);
        pool.run(4, &|_| {
            calls.fetch_add(1, Ordering::Relaxed);
        });
        assert_eq!(calls.load(Ordering::Relaxed), 4);
    }
}
