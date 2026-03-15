use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

pub struct BackgroundWorker {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl BackgroundWorker {
    pub fn spawn<F>(interval: Duration, job: F) -> Self
    where
        F: Fn() + Send + 'static,
    {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            while !stop_clone.load(Ordering::Relaxed) {
                thread::park_timeout(interval);
                if stop_clone.load(Ordering::Relaxed) {
                    break;
                }
                job();
            }
        });

        BackgroundWorker {
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for BackgroundWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            handle.thread().unpark(); // wake the thread if it's sleeping
            handle.join().unwrap(); // block until the thread exits
        }
    }
}
