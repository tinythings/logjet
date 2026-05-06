//! Reads exported Perfetto SQLite databases.
//!
//! Provides typed models for all tables needed by the OTel mappers and a
//! `PerfettoDb` struct that opens the exported database and exposes query
//! methods.

#![allow(dead_code)]

use std::path::Path;

// ── Typed models ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PerfettoSlice {
    pub id: i64,
    pub ts: i64,
    pub dur: i64,
    pub name: Option<String>,
    pub parent_id: Option<i64>,
    pub track_id: i64,
    pub arg_set_id: Option<i64>,
    pub depth: i32,
}

#[derive(Debug, Clone)]
pub struct PerfettoFlow {
    pub id: i64,
    pub slice_out: i64,
    pub slice_in: i64,
}

#[derive(Debug, Clone)]
pub struct PerfettoProcess {
    pub upid: i64,
    pub name: Option<String>,
    pub pid: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct PerfettoThread {
    pub utid: i64,
    pub name: Option<String>,
    pub tid: Option<i64>,
    pub upid: Option<i64>,
    pub is_main_thread: bool,
}

#[derive(Debug, Clone)]
pub struct PerfettoTrack {
    pub id: i64,
    pub name: Option<String>,
    pub track_type: Option<String>,
    pub utid: Option<i64>,
    pub upid: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct PerfettoArg {
    pub arg_set_id: i64,
    pub key: String,
    pub string_value: Option<String>,
    pub int_value: Option<i64>,
    pub real_value: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct PerfettoClockSnapshot {
    pub ts: i64,
    pub clock_value: i64,
}

// ── Database reader ──────────────────────────────────────────────────────────

pub struct PerfettoDb {
    pub(crate) conn: rusqlite::Connection,
}

impl PerfettoDb {
    /// Opens an exported Perfetto SQLite database.
    pub fn open(path: &Path) -> Result<Self, String> {
        let conn = rusqlite::Connection::open(path)
            .map_err(|err| format!("failed to open exported SQLite DB {}: {err}", path.display()))?;

        // Enable WAL mode for better read concurrency.
        let _ = conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA read_uncommitted=1;");

        Ok(Self { conn })
    }

    /// Reads all slices ordered by ts.
    pub fn read_slices(&self) -> Result<Vec<PerfettoSlice>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, ts, dur, name, parent_id, track_id, arg_set_id, depth
                 FROM slice
                 ORDER BY ts",
            )
            .map_err(|err| format!("failed to prepare slice query: {err}"))?;

        let rows = stmt
            .query_map([], |row| {
                Ok(PerfettoSlice {
                    id: row.get(0)?,
                    ts: row.get(1)?,
                    dur: row.get(2)?,
                    name: row.get(3)?,
                    parent_id: row.get(4)?,
                    track_id: row.get(5)?,
                    arg_set_id: row.get(6)?,
                    depth: row.get(7)?,
                })
            })
            .map_err(|err| format!("failed to query slices: {err}"))?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|err| format!("failed to read slice row: {err}"))?);
        }
        Ok(out)
    }

    /// Reads all flows.
    pub fn read_flows(&self) -> Result<Vec<PerfettoFlow>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, slice_out, slice_in FROM flow ORDER BY id")
            .map_err(|err| format!("failed to prepare flow query: {err}"))?;

        let rows = stmt
            .query_map([], |row| {
                Ok(PerfettoFlow { id: row.get(0)?, slice_out: row.get(1)?, slice_in: row.get(2)? })
            })
            .map_err(|err| format!("failed to query flows: {err}"))?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|err| format!("failed to read flow row: {err}"))?);
        }
        Ok(out)
    }

    /// Reads all processes.
    pub fn read_processes(&self) -> Result<Vec<PerfettoProcess>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT upid, name, pid FROM process ORDER BY upid")
            .map_err(|err| format!("failed to prepare process query: {err}"))?;

        let rows = stmt
            .query_map([], |row| {
                Ok(PerfettoProcess { upid: row.get(0)?, name: row.get(1)?, pid: row.get(2)? })
            })
            .map_err(|err| format!("failed to query processes: {err}"))?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|err| format!("failed to read process row: {err}"))?);
        }
        Ok(out)
    }

    /// Reads all threads.
    pub fn read_threads(&self) -> Result<Vec<PerfettoThread>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT utid, name, tid, upid, is_main_thread FROM thread ORDER BY utid")
            .map_err(|err| format!("failed to prepare thread query: {err}"))?;

        let rows = stmt
            .query_map([], |row| {
                Ok(PerfettoThread {
                    utid: row.get(0)?,
                    name: row.get(1)?,
                    tid: row.get(2)?,
                    upid: row.get(3)?,
                    is_main_thread: row.get::<_, Option<i32>>(4)?.unwrap_or(0) != 0,
                })
            })
            .map_err(|err| format!("failed to query threads: {err}"))?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|err| format!("failed to read thread row: {err}"))?);
        }
        Ok(out)
    }

    /// Reads all tracks.
    pub fn read_tracks(&self) -> Result<Vec<PerfettoTrack>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, type, utid, upid FROM track ORDER BY id")
            .map_err(|err| format!("failed to prepare track query: {err}"))?;

        let rows = stmt
            .query_map([], |row| {
                Ok(PerfettoTrack {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    track_type: row.get(2)?,
                    utid: row.get(3)?,
                    upid: row.get(4)?,
                })
            })
            .map_err(|err| format!("failed to query tracks: {err}"))?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|err| format!("failed to read track row: {err}"))?);
        }
        Ok(out)
    }

    /// Reads args for the given arg_set_ids. Pass an empty slice to read all.
    pub fn read_args(&self, arg_set_ids: &[i64]) -> Result<Vec<PerfettoArg>, String> {
        if arg_set_ids.is_empty() {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT arg_set_id, flat_key, string_value, int_value, real_value
                     FROM args
                     ORDER BY arg_set_id, flat_key",
                )
                .map_err(|err| format!("failed to prepare args query: {err}"))?;

            let rows = stmt
                .query_map([], |row| {
                    Ok(PerfettoArg {
                        arg_set_id: row.get(0)?,
                        key: row.get(1)?,
                        string_value: row.get(2)?,
                        int_value: row.get(3)?,
                        real_value: row.get(4)?,
                    })
                })
                .map_err(|err| format!("failed to query args: {err}"))?;

            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(|err| format!("failed to read arg row: {err}"))?);
            }
            return Ok(out);
        }

        let placeholders = arg_set_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT arg_set_id, flat_key, string_value, int_value, real_value
             FROM args
             WHERE arg_set_id IN ({placeholders})
             ORDER BY arg_set_id, flat_key"
        );

        let mut stmt = self.conn.prepare(&sql).map_err(|err| format!("failed to prepare args IN query: {err}"))?;

        let params: Vec<&dyn rusqlite::types::ToSql> = arg_set_ids
            .iter()
            .map(|id| id as &dyn rusqlite::types::ToSql)
            .collect();

        let rows = stmt
            .query_map(params.as_slice(), |row| {
                Ok(PerfettoArg {
                    arg_set_id: row.get(0)?,
                    key: row.get(1)?,
                    string_value: row.get(2)?,
                    int_value: row.get(3)?,
                    real_value: row.get(4)?,
                })
            })
            .map_err(|err| format!("failed to query args by id: {err}"))?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|err| format!("failed to read arg row: {err}"))?);
        }
        Ok(out)
    }

    /// Reads clock snapshots ordered by ts. Returns entries for all clock_ids.
    pub fn read_clock_snapshots(&self) -> Result<Vec<PerfettoClockSnapshot>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT ts, clock_value
                 FROM clock_snapshot
                 WHERE clock_name = 'REALTIME'
                 ORDER BY ts",
            )
            .map_err(|err| format!("failed to prepare clock_snapshot query: {err}"))?;

        let rows = stmt
            .query_map([], |row| Ok(PerfettoClockSnapshot { ts: row.get(0)?, clock_value: row.get(1)? }))
            .map_err(|err| format!("failed to query clock_snapshots: {err}"))?;

        let mut out = Vec::new();
        for row in rows {
            match row {
                Ok(entry) => out.push(entry),
                Err(err) => return Err(format!("failed to read clock_snapshot row: {err}")),
            }
        }
        Ok(out)
    }
}
