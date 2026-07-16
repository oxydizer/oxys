use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use super::{PackageError, Result};

pub(crate) fn worker_count() -> usize {
    worker_count_for(
        std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1),
    )
}

fn worker_count_for(available: usize) -> usize {
    available.saturating_sub(1).max(1)
}

pub(crate) fn try_for_each<T, F>(items: &[T], operation: F) -> Result<()>
where
    T: Sync,
    F: Fn(&T) -> Result<()> + Sync,
{
    if items.is_empty() {
        return Ok(());
    }
    let workers = worker_count().min(items.len());
    let next = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    let failure = Mutex::new(None);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    if failed.load(Ordering::Acquire) {
                        break;
                    }
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(index) else {
                        break;
                    };
                    if let Err(error) = operation(item) {
                        if failed
                            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                            .is_ok()
                            && let Ok(mut failure) = failure.lock()
                        {
                            *failure = Some(error);
                        }
                        break;
                    }
                }
            });
        }
    });
    failure
        .into_inner()
        .map_err(|_| PackageError::invalid("parallel worker error lock was poisoned"))?
        .map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserves_one_cpu_but_never_returns_zero_workers() {
        assert_eq!(worker_count_for(1), 1);
        assert_eq!(worker_count_for(2), 1);
        assert_eq!(worker_count_for(8), 7);
    }
}
