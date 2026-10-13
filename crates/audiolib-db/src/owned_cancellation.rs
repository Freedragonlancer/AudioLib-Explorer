// crates/audiolib-db/src/owned_cancellation.rs
//! Cancellation scoped to a caller-owned connection, never a live writer or reader pool.
use crate::{Error, Result, db_error};
use rusqlite::Connection;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, SyncSender},
};
use std::time::Duration;

struct Completion(SyncSender<()>);
impl Drop for Completion {
    fn drop(&mut self) {
        let _ = self.0.try_send(());
    }
}

/// Interrupt owned SQLite work and join its cancellation watcher before returning.
/// The progress hook catches cancellation between statements; the interrupt handle
/// also reaches statements already running inside SQLite. No connection escapes.
pub(crate) fn run<T>(
    conn: &mut Connection,
    cancel: &AtomicBool,
    work: impl FnOnce(&mut Connection) -> Result<T>,
) -> Result<T> {
    if cancel.load(Ordering::Acquire) {
        return Err(Error::Cancelled);
    }
    let observed = Arc::new(AtomicBool::new(false));
    let progress = observed.clone();
    conn.progress_handler(2000, Some(move || progress.load(Ordering::Acquire)))
        .map_err(db_error)?;
    let interrupt = conn.get_interrupt_handle();
    let result = std::thread::scope(|scope| {
        let (done, waiting) = mpsc::sync_channel(1);
        let _completion = Completion(done);
        std::thread::Builder::new()
            .name("audiolib-owned-cancel".into())
            .spawn_scoped(scope, move || {
                loop {
                    if cancel.load(Ordering::Acquire) {
                        observed.store(true, Ordering::Release);
                        // Repeat until work has drained: a cancellation between two
                        // statements must also interrupt the next long statement.
                        interrupt.interrupt();
                    }
                    match waiting.recv_timeout(Duration::from_millis(20)) {
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        _ => break,
                    }
                }
            })?;
        let result = work(conn);
        if cancel.load(Ordering::Acquire) {
            Err(Error::Cancelled)
        } else {
            result
        }
    });
    conn.progress_handler(0, None::<fn() -> bool>)
        .map_err(db_error)?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn cancellation_interrupts_running_sql_and_joins_the_watcher() {
        let mut conn = Connection::open_in_memory().unwrap();
        let cancel = AtomicBool::new(false);
        let started = Instant::now();
        std::thread::scope(|scope| {
            let (entered, waiting) = mpsc::sync_channel(1);
            let flag = &cancel;
            scope.spawn(move || {
                waiting.recv().unwrap();
                std::thread::sleep(Duration::from_millis(30));
                flag.store(true, Ordering::Release);
            });
            let result = run(&mut conn, &cancel, |conn| {
                entered.send(()).unwrap();
                conn.query_row(
                    "WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM n WHERE x<1000000000) SELECT SUM(x) FROM n",
                    [],
                    |row| row.get::<_, i64>(0),
                ).map_err(db_error)
            });
            assert!(matches!(result, Err(Error::Cancelled)));
        });
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(
            conn.query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}
