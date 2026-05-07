//! Reads Perfetto data via RPC instead of SQLite export.
#![allow(dead_code)]

use crate::rpc_client::{CellValue, QueryResult, RpcClient};
use crate::sqlite_reader::{
    PerfettoClockSnapshot, PerfettoFtraceEvent, PerfettoInstant, PerfettoProcess, PerfettoSchedSlice, PerfettoSlice, PerfettoSpuriousWakeup,
    PerfettoThread, PerfettoThreadState,
};

pub struct RpcReader {
    client: RpcClient,
}

impl RpcReader {
    pub fn new(client: RpcClient) -> Self {
        Self { client }
    }

    fn col_idx(columns: &[String], name: &str) -> Option<usize> {
        columns.iter().position(|c| c == name)
    }

    fn query(&mut self, sql: &str) -> std::io::Result<QueryResult> {
        self.client.query(sql)
    }

    pub fn read_slices(&mut self) -> std::io::Result<Vec<PerfettoSlice>> {
        let r = self.query("SELECT id, ts, dur, name, parent_id, track_id, arg_set_id, depth FROM slice ORDER BY ts")?;
        let (id, ts, dur, name, pid, tid, aid, depth) = (
            Self::col_idx(&r.column_names, "id"),
            Self::col_idx(&r.column_names, "ts"),
            Self::col_idx(&r.column_names, "dur"),
            Self::col_idx(&r.column_names, "name"),
            Self::col_idx(&r.column_names, "parent_id"),
            Self::col_idx(&r.column_names, "track_id"),
            Self::col_idx(&r.column_names, "arg_set_id"),
            Self::col_idx(&r.column_names, "depth"),
        );
        Ok(r.rows
            .iter()
            .map(|row| PerfettoSlice {
                id: i64_val(row, id),
                ts: i64_val(row, ts),
                dur: i64_val(row, dur),
                name: str_val(row, name),
                parent_id: opt_i64_val(row, pid),
                track_id: i64_val(row, tid),
                arg_set_id: opt_i64_val(row, aid),
                depth: i32_val(row, depth),
            })
            .collect())
    }

    pub fn read_sched_slices(&mut self) -> std::io::Result<Vec<PerfettoSchedSlice>> {
        let r = self.query("SELECT id, ts, dur, utid, ucpu, end_state FROM sched_slice ORDER BY ts")?;
        let (id, ts, dur, utid, cpu, es) = (
            Self::col_idx(&r.column_names, "id"),
            Self::col_idx(&r.column_names, "ts"),
            Self::col_idx(&r.column_names, "dur"),
            Self::col_idx(&r.column_names, "utid"),
            Self::col_idx(&r.column_names, "ucpu"),
            Self::col_idx(&r.column_names, "end_state"),
        );
        Ok(r.rows
            .iter()
            .map(|row| PerfettoSchedSlice {
                id: i64_val(row, id),
                ts: i64_val(row, ts),
                dur: i64_val(row, dur),
                utid: i64_val(row, utid),
                cpu: i64_val(row, cpu),
                end_state: str_val(row, es),
            })
            .collect())
    }

    pub fn read_thread_states(&mut self) -> std::io::Result<Vec<PerfettoThreadState>> {
        let r = self.query("SELECT id, ts, dur, utid, state, io_wait, blocked_function, waker_utid, cpu FROM thread_state ORDER BY ts")?;
        let (id, ts, dur, utid, st, iw, bf, wu, cpu) = (
            Self::col_idx(&r.column_names, "id"),
            Self::col_idx(&r.column_names, "ts"),
            Self::col_idx(&r.column_names, "dur"),
            Self::col_idx(&r.column_names, "utid"),
            Self::col_idx(&r.column_names, "state"),
            Self::col_idx(&r.column_names, "io_wait"),
            Self::col_idx(&r.column_names, "blocked_function"),
            Self::col_idx(&r.column_names, "waker_utid"),
            Self::col_idx(&r.column_names, "cpu"),
        );
        Ok(r.rows
            .iter()
            .map(|row| PerfettoThreadState {
                id: i64_val(row, id),
                ts: i64_val(row, ts),
                dur: i64_val(row, dur),
                utid: i64_val(row, utid),
                state: str_val(row, st),
                io_wait: opt_i64_val(row, iw).map(|v| v != 0),
                blocked_function: str_val(row, bf),
                waker_utid: opt_i64_val(row, wu),
                cpu: opt_i64_val(row, cpu),
            })
            .collect())
    }

    pub fn read_ftrace_events(&mut self) -> std::io::Result<Vec<PerfettoFtraceEvent>> {
        let r = self.query("SELECT id, ts, name, cpu, utid FROM ftrace_event ORDER BY ts")?;
        let (id, ts, name, cpu, utid) = (
            Self::col_idx(&r.column_names, "id"),
            Self::col_idx(&r.column_names, "ts"),
            Self::col_idx(&r.column_names, "name"),
            Self::col_idx(&r.column_names, "cpu"),
            Self::col_idx(&r.column_names, "utid"),
        );
        Ok(r.rows
            .iter()
            .map(|row| PerfettoFtraceEvent {
                id: i64_val(row, id),
                ts: i64_val(row, ts),
                name: str_val(row, name),
                cpu: opt_i64_val(row, cpu),
                utid: opt_i64_val(row, utid),
            })
            .collect())
    }

    pub fn read_spurious_wakeups(&mut self) -> std::io::Result<Vec<PerfettoSpuriousWakeup>> {
        let r = self.query("SELECT id, ts, utid, waker_utid FROM spurious_sched_wakeup ORDER BY ts")?;
        let (id, ts, utid, wu) = (
            Self::col_idx(&r.column_names, "id"),
            Self::col_idx(&r.column_names, "ts"),
            Self::col_idx(&r.column_names, "utid"),
            Self::col_idx(&r.column_names, "waker_utid"),
        );
        Ok(r.rows
            .iter()
            .map(|row| PerfettoSpuriousWakeup {
                id: i64_val(row, id),
                ts: i64_val(row, ts),
                utid: opt_i64_val(row, utid),
                waker_utid: opt_i64_val(row, wu),
            })
            .collect())
    }

    pub fn read_instants(&mut self) -> std::io::Result<Vec<PerfettoInstant>> {
        let r = self.query("SELECT ts, track_id, name FROM instant ORDER BY ts")?;
        let (ts, tid, name) =
            (Self::col_idx(&r.column_names, "ts"), Self::col_idx(&r.column_names, "track_id"), Self::col_idx(&r.column_names, "name"));
        Ok(r.rows.iter().map(|row| PerfettoInstant { ts: i64_val(row, ts), track_id: i64_val(row, tid), name: str_val(row, name) }).collect())
    }

    pub fn read_threads(&mut self) -> std::io::Result<Vec<PerfettoThread>> {
        let r = self.query("SELECT utid, name, tid, upid, is_main_thread FROM thread ORDER BY utid")?;
        let (utid, name, tid, upid, imt) = (
            Self::col_idx(&r.column_names, "utid"),
            Self::col_idx(&r.column_names, "name"),
            Self::col_idx(&r.column_names, "tid"),
            Self::col_idx(&r.column_names, "upid"),
            Self::col_idx(&r.column_names, "is_main_thread"),
        );
        Ok(r.rows
            .iter()
            .map(|row| PerfettoThread {
                utid: i64_val(row, utid),
                name: str_val(row, name),
                tid: opt_i64_val(row, tid),
                upid: opt_i64_val(row, upid),
                is_main_thread: opt_i64_val(row, imt).unwrap_or(0) != 0,
            })
            .collect())
    }

    pub fn read_processes(&mut self) -> std::io::Result<Vec<PerfettoProcess>> {
        let r = self.query("SELECT upid, name, pid FROM process ORDER BY upid")?;
        let (upid, name, pid) =
            (Self::col_idx(&r.column_names, "upid"), Self::col_idx(&r.column_names, "name"), Self::col_idx(&r.column_names, "pid"));
        Ok(r.rows.iter().map(|row| PerfettoProcess { upid: i64_val(row, upid), name: str_val(row, name), pid: opt_i64_val(row, pid) }).collect())
    }

    pub fn read_clock_snapshots(&mut self) -> std::io::Result<Vec<PerfettoClockSnapshot>> {
        let r = self.query("SELECT ts, clock_value FROM clock_snapshot WHERE clock_name = 'REALTIME' ORDER BY ts")?;
        let (ts, cv) = (Self::col_idx(&r.column_names, "ts"), Self::col_idx(&r.column_names, "clock_value"));
        Ok(r.rows.iter().map(|row| PerfettoClockSnapshot { ts: i64_val(row, ts), clock_value: i64_val(row, cv) }).collect())
    }
}

fn i64_val(row: &[CellValue], idx: Option<usize>) -> i64 {
    idx.and_then(|i| row.get(i)).and_then(cell_i64).unwrap_or(0)
}
fn i32_val(row: &[CellValue], idx: Option<usize>) -> i32 {
    idx.and_then(|i| row.get(i)).and_then(cell_i64).unwrap_or(0) as i32
}
fn opt_i64_val(row: &[CellValue], idx: Option<usize>) -> Option<i64> {
    idx.and_then(|i| row.get(i)).and_then(cell_i64)
}
fn str_val(row: &[CellValue], idx: Option<usize>) -> Option<String> {
    idx.and_then(|i| row.get(i)).and_then(cell_str)
}

fn cell_i64(cv: &CellValue) -> Option<i64> {
    match cv {
        CellValue::Varint(v) => Some(*v),
        CellValue::Float64(f) => Some(*f as i64),
        _ => None,
    }
}
fn cell_str(cv: &CellValue) -> Option<String> {
    match cv {
        CellValue::String(s) => Some(s.clone()),
        CellValue::Null => None,
        _ => None,
    }
}
