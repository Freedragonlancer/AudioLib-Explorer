// crates/audiolib-db/tests/smv2/driver.rs
//! Pluggable schema 8→9 migration entrypoint for the MG cases.
//!
//! The migration lives in the server migration branch. Until it lands, every MG test
//! that needs it is `#[ignore]`d with [`PENDING`]. To run them, point
//! `AUDIOLIB_SMV2_MIGRATE_CMD` at an executable implementing the protocol below and
//! run `cargo test -p audiolib-db --test smv2_mg -- --ignored`.
//!
//! Protocol (one process per call):
//!   `<cmd> [prefix args] <action> --db <path> [--sql-trace <file>]`
//!   action ∈ startup | start | pause | resume | cancel | status | rollback |
//!            main_search | path_search
//!   main_search / path_search read AUDIOLIB_SMV2_QUERY and report {"ids":[..]}
//!   using the s9 main-table-only and path-table-only scopes respectively.
//! The last stdout line is a JSON [`Report`]. Exit 0 unless the process was aborted
//! by a fault. `pause` / `cancel` act on a migration left `paused` by an earlier call
//! (a single CLI process cannot be paused from outside; `STOP_AFTER_BATCHES` pauses).
//!
//! Environment understood by the entrypoint (fault injection, test-only):
//!   AUDIOLIB_MIGRATION_FAULT  abort:batch_boundary:<n> | abort:mid_batch:<n> |
//!                             abort:optimize | abort:switch_tx | sqlite_full:batch:<n> |
//!                             backup_fail
//!     abort:* must terminate the process without unwinding (std::process::abort),
//!     emulating kill -9 at that point.
//!   AUDIOLIB_MIGRATION_FREE_BYTES      override the free-space probe (MG-02, MG-13)
//!   AUDIOLIB_MIGRATION_STOP_AFTER_BATCHES  pause after n committed batches
//!   AUDIOLIB_MIGRATION_BATCH_ROWS      initial batch size (default chosen by server)
//!   AUDIOLIB_MIGRATION_SLOW_BATCH_MS   sleep inside batch k (format `<k>:<ms>`), MG-11
//!   AUDIOLIB_DEFER_SCHEMA9             honoured by `startup` (MG-16)
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    process::Command,
};

pub const ENV_CMD: &str = "AUDIOLIB_SMV2_MIGRATE_CMD";
pub const ENV_PREFIX: &str = "AUDIOLIB_SMV2_MIGRATE_PREFIX_ARGS";
pub const PENDING: &str = "pending server migration branch (set AUDIOLIB_SMV2_MIGRATE_CMD)";

pub struct Driver {
    program: PathBuf,
    prefix: Vec<String>,
}

#[derive(Debug)]
pub struct Outcome {
    pub code: Option<i32>,
    pub report: Value,
    pub stdout: String,
    pub stderr: String,
}

impl Outcome {
    pub fn aborted(&self) -> bool {
        self.code != Some(0)
    }
    pub fn state(&self) -> &str {
        self.report["state"].as_str().unwrap_or("")
    }
    pub fn error_code(&self) -> Option<&str> {
        self.report["error_code"].as_str()
    }
    pub fn batch_ms(&self) -> Vec<f64> {
        self.report["batches"]
            .as_array()
            .map(|b| b.iter().filter_map(|x| x["ms"].as_f64()).collect())
            .unwrap_or_default()
    }
    pub fn batch_rows(&self) -> Vec<u64> {
        self.report["batches"]
            .as_array()
            .map(|b| b.iter().filter_map(|x| x["rows"].as_u64()).collect())
            .unwrap_or_default()
    }
}

impl Driver {
    pub fn from_env() -> Option<Driver> {
        let program = std::env::var_os(ENV_CMD)?;
        let prefix = std::env::var(ENV_PREFIX)
            .map(|s| s.split_whitespace().map(String::from).collect())
            .unwrap_or_default();
        Some(Driver {
            program: program.into(),
            prefix,
        })
    }

    /// The driver, or a panic carrying [`PENDING`] (only reached under `--ignored`).
    pub fn require() -> Driver {
        Driver::from_env().unwrap_or_else(|| panic!("{PENDING}"))
    }

    pub fn run(&self, action: &str, db: &Path, env: &[(&str, String)]) -> Outcome {
        self.run_traced(action, db, env, None)
    }

    pub fn run_traced(
        &self,
        action: &str,
        db: &Path,
        env: &[(&str, String)],
        sql_trace: Option<&Path>,
    ) -> Outcome {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.prefix).arg(action).arg("--db").arg(db);
        if let Some(trace) = sql_trace {
            cmd.arg("--sql-trace").arg(trace);
        }
        for (k, v) in env {
            cmd.env(k, v);
        }
        let out = cmd.output().expect("spawn migration entrypoint");
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let report = stdout
            .lines()
            .rev()
            .find_map(|l| serde_json::from_str::<Value>(l.trim()).ok())
            .unwrap_or(Value::Null);
        Outcome {
            code: out.status.code(),
            report,
            stdout,
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    /// Run `start`, then `resume` until done (bounded), returning the last outcome.
    pub fn migrate_to_done(&self, db: &Path) -> Outcome {
        let mut last = self.run("start", db, &[]);
        for _ in 0..10 {
            if last.state() == "done" {
                break;
            }
            last = self.run("resume", db, &[]);
        }
        assert_eq!(last.state(), "done", "migration did not finish: {last:?}");
        last
    }
}
