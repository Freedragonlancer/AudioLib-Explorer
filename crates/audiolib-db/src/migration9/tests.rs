// crates/audiolib-db/src/migration9/tests.rs
//! Control admission at the final backfill/publication boundary.
use super::*;
use std::sync::mpsc;

#[test]
fn control_requests_remain_excluded_by_library_maintenance() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.db");
    drop(crate::SqliteProvider::open(&path).unwrap());
    let mut conn = open(&path, true).unwrap();
    storage::prepare(&mut conn).unwrap();
    let _administration = AdministrationLease::exclusive(&path).unwrap();
    assert!(matches!(
        storage::request_control(&path, Action::Pause),
        Err(Error::Busy(_))
    ));
    assert_eq!(
        conn.query_row("SELECT count(*) FROM v9_control", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn publication_drains_admitted_control_before_taking_exclusive_administration() {
    for action in [Action::Pause, Action::Cancel] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.db");
        drop(crate::SqliteProvider::open(&path).unwrap());
        let mut conn = open(&path, true).unwrap();
        storage::prepare(&mut conn).unwrap();
        let mut saved = load(&path, &conn).unwrap();
        saved.report.state = State::Running;
        save(&path, &saved).unwrap();

        // Hold exactly the leases of a control request admitted while the last
        // batch owns SQLite's writer. Publication must let that request commit
        // and release its shared administration lease before attempting upgrade.
        let control = storage::ControlLease::acquire(&path).unwrap();
        let administration = AdministrationLease::shared(&path).unwrap();
        std::thread::scope(|scope| {
            let (started, starting) = mpsc::channel();
            let (finished, completion) = mpsc::channel();
            let path = &path;
            scope.spawn(move || {
                let mut publisher = open(path, true).unwrap();
                started.send(()).unwrap();
                let result =
                    storage::publish(path, &mut publisher, &mut saved, &AtomicBool::new(false));
                finished.send((result, saved.report)).unwrap();
            });
            starting.recv_timeout(Duration::from_secs(5)).unwrap();
            let pending = completion.recv_timeout(Duration::from_millis(100));
            assert!(
                matches!(pending, Err(mpsc::RecvTimeoutError::Timeout)),
                "publication overtook an admitted control request: {pending:?}"
            );
            conn.execute(
                "INSERT INTO v9_control VALUES(1,?1,'test')",
                [if action == Action::Cancel {
                    "cancel"
                } else {
                    "pause"
                }],
            )
            .unwrap();
            drop(administration);
            drop(control);
            let (result, report) = completion.recv_timeout(Duration::from_secs(5)).unwrap();
            result.unwrap();
            assert_eq!(
                report.state,
                if action == Action::Cancel {
                    State::Cancelled
                } else {
                    State::Paused
                }
            );
            assert_eq!(report.phases["switch"], 0.0);
            assert_eq!(version(&conn).unwrap(), 8);
        });
    }
}
