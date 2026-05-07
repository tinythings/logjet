//! RPC client for Perfetto trace_processor `server stdio` mode.
//! Wire format: `[0x0a] [varint len] [serialized TraceProcessorRpc]`

#![allow(dead_code)]

use std::io::Write;

// Varint

fn read_varint(buf: &[u8], pos: &mut usize) -> Option<u64> {
    let (mut value, mut shift) = (0u64, 0u32);
    loop {
        if *pos >= buf.len() {
            return None;
        }
        let byte = buf[*pos];
        *pos += 1;
        value |= ((byte & 0x7F) as u64) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
}

fn write_varint(mut value: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(10);
    while value >= 0x80 {
        out.push((value as u8 & 0x7F) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
    out
}

// Wire type helpers

fn write_tagged_varint(field: u64, value: u64) -> Vec<u8> {
    let mut out = write_varint(field << 3);
    out.extend(write_varint(value));
    out
}

fn write_tagged_bytes(field: u64, data: &[u8]) -> Vec<u8> {
    let mut out = write_varint((field << 3) | 2);
    out.extend(write_varint(data.len() as u64));
    out.extend_from_slice(data);
    out
}

fn write_tagged_str(field: u64, value: &str) -> Vec<u8> {
    write_tagged_bytes(field, value.as_bytes())
}

// Protobuf field reader

#[derive(Debug, Clone)]
enum FieldValue<'a> {
    Varint(u64),
    LengthDelimited(&'a [u8]),
}

fn read_next_tag<'a>(buf: &'a [u8], pos: &mut usize) -> Option<(u64, FieldValue<'a>)> {
    let tag_raw = read_varint(buf, pos)?;
    match (tag_raw & 0x07) as u8 {
        0 => read_varint(buf, pos).map(|v| (tag_raw >> 3, FieldValue::Varint(v))),
        2 => {
            let len = read_varint(buf, pos)? as usize;
            (*pos + len <= buf.len()).then(|| {
                let data = &buf[*pos..*pos + len];
                *pos += len;
                (tag_raw >> 3, FieldValue::LengthDelimited(data))
            })
        }
        _ => {
            let _ = read_varint(buf, pos);
            Some((tag_raw >> 3, FieldValue::Varint(0)))
        }
    }
}

// Messages

const TPM_QUERY_STREAMING: u64 = 3;
const CELL_NULL: i32 = 1;
const CELL_VARINT: i32 = 2;
const CELL_FLOAT64: i32 = 3;
const CELL_STRING: i32 = 4;

/// Decoded query result from trace_processor, containing column names, error
/// message, and rows of `CellValue` data.
#[derive(Debug, Clone)]
pub struct QueryResult {
    pub column_names: Vec<String>,
    pub error: Option<String>,
    pub rows: Vec<Vec<CellValue>>,
    pub is_last: bool,
}

/// One cell in a query result row.
#[derive(Debug, Clone)]
pub enum CellValue {
    Null,
    Varint(i64),
    Float64(f64),
    String(String),
}

/// Builds a length-delimited `TraceProcessorRpc` query request frame ready to
/// write to the trace_processor's stdin.
pub fn build_query_request(seq: u64, sql: &str) -> Vec<u8> {
    let query_args = write_tagged_str(1, sql);
    let rpc = [write_tagged_varint(1, seq), write_tagged_varint(2, TPM_QUERY_STREAMING), write_tagged_bytes(103, &query_args)].concat();
    [&[0x0au8][..], &write_varint(rpc.len() as u64), &rpc].concat()
}

/// Parses a length-delimited `TraceProcessorRpc` response frame. Returns
/// `Some(QueryResult)` if the frame is a `TPM_QUERY_STREAMING` response,
/// or `None` otherwise.
pub fn parse_response(rpc_bytes: &[u8]) -> Option<QueryResult> {
    let (mut pos, mut qr_bytes, mut is_resp) = (0, None, false);
    while pos < rpc_bytes.len() {
        let (field, value) = read_next_tag(rpc_bytes, &mut pos)?;
        match field {
            3 => is_resp = matches!(value, FieldValue::Varint(v) if v == TPM_QUERY_STREAMING),
            203 => {
                if let FieldValue::LengthDelimited(data) = value {
                    qr_bytes = Some(data);
                }
            }
            _ => {}
        }
    }
    is_resp.then_some(()).and_then(|_| decode_query_result(qr_bytes?))
}

fn decode_query_result(bytes: &[u8]) -> Option<QueryResult> {
    let (mut pos, mut column_names, mut error, mut all_rows) = (0, Vec::new(), None, Vec::new());
    while pos < bytes.len() {
        let (field, value) = read_next_tag(bytes, &mut pos)?;
        match (field, value) {
            (1, FieldValue::LengthDelimited(d)) => column_names.push(String::from_utf8_lossy(d).to_string()),
            (2, FieldValue::LengthDelimited(d)) => error = Some(String::from_utf8_lossy(d).to_string()),
            (3, FieldValue::LengthDelimited(d)) => all_rows.extend(decode_cells_batch(d)?),
            _ => {}
        }
    }
    Some(QueryResult { column_names, error, rows: all_rows, is_last: false })
}

fn decode_cells_batch(data: &[u8]) -> Option<Vec<Vec<CellValue>>> {
    let (mut pos, mut cell_types, mut varint_cells, mut float64_cells, mut string_cells) = (0, Vec::new(), Vec::new(), Vec::new(), Vec::new());
    while pos < data.len() {
        let (field, value) = read_next_tag(data, &mut pos)?;
        match (field, value) {
            (1, FieldValue::LengthDelimited(d)) => cell_types = read_packed_varints(d).into_iter().map(|v| v as i32).collect(),
            (2, FieldValue::LengthDelimited(d)) => varint_cells = read_packed_varints(d).into_iter().map(decode_zigzag).collect(),
            (3, FieldValue::LengthDelimited(d)) => {
                float64_cells = d.chunks(8).filter_map(|c| <[u8; 8]>::try_from(c).ok()).map(f64::from_le_bytes).collect()
            }
            (5, FieldValue::LengthDelimited(d)) => string_cells = String::from_utf8_lossy(d).split('\0').map(String::from).collect(),
            _ => {}
        }
    }
    let ncols = cell_types.len();
    if ncols == 0 {
        return Some(Vec::new());
    }
    let nrows = cell_types.len() / ncols;
    let (mut rows, mut vi, mut fi, mut si) = (Vec::with_capacity(nrows), 0usize, 0usize, 0usize);
    for ri in 0..nrows {
        let mut row = Vec::with_capacity(ncols);
        for ci in 0..ncols {
            row.push(match cell_types[ri * ncols + ci] {
                CELL_VARINT => {
                    vi += 1;
                    varint_cells.get(vi - 1).map(|&v| CellValue::Varint(v)).unwrap_or(CellValue::Null)
                }
                CELL_FLOAT64 => {
                    fi += 1;
                    float64_cells.get(fi - 1).map(|&v| CellValue::Float64(v)).unwrap_or(CellValue::Null)
                }
                CELL_STRING => {
                    si += 1;
                    string_cells.get(si - 1).map(|s| CellValue::String(s.clone())).unwrap_or(CellValue::Null)
                }
                _ => CellValue::Null,
            });
        }
        rows.push(row);
    }
    Some(rows)
}

fn decode_zigzag(v: u64) -> i64 {
    ((v >> 1) as i64) ^ -((v & 1) as i64)
}

fn read_packed_varints(data: &[u8]) -> Vec<u64> {
    let (mut p, mut out) = (0, Vec::new());
    while p < data.len() {
        if let Some(v) = read_varint(data, &mut p) {
            out.push(v);
        } else {
            break;
        }
    }
    out
}

// RPC Client

/// Connected trace_processor child process in `server stdio` mode.
pub struct RpcClient {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    stdout: std::io::BufReader<std::process::ChildStdout>,
    next_seq: u64,
}

impl RpcClient {
    /// Spawns `trace_processor server stdio <trace_file>` and returns a client
    /// for sending SQL queries over stdin/stdout.
    pub fn connect(tp_path: &std::path::Path, trace_file: &std::path::Path) -> std::io::Result<Self> {
        let mut child = std::process::Command::new(tp_path)
            .args(["server", "stdio"])
            .arg(trace_file)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .spawn()?;
        let stdin = child.stdin.take().expect("stdin");
        let stdout = std::io::BufReader::new(child.stdout.take().expect("stdout"));
        Ok(Self { child, stdin, stdout, next_seq: 0 })
    }

    /// Sends a SQL query and collects all response batches into a single
    /// `QueryResult`. Each batch is parsed from the length-delimited framing.
    pub fn query(&mut self, sql: &str) -> std::io::Result<QueryResult> {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.stdin.write_all(&build_query_request(seq, sql))?;
        self.stdin.flush()?;
        let (mut result, mut got_columns) = (QueryResult { column_names: vec![], error: None, rows: vec![], is_last: false }, false);
        loop {
            let frame = self.read_frame()?;
            if let Some(qr) = parse_response(&frame) {
                if !qr.column_names.is_empty() && !got_columns {
                    result.column_names = qr.column_names.clone();
                    got_columns = true;
                }
                if let Some(ref err) = qr.error
                    && !err.is_empty()
                {
                    result.error = Some(err.clone());
                    return Ok(result);
                }
                result.rows.extend(qr.rows);
                result.column_names = qr.column_names.clone();
                if qr.is_last {
                    result.is_last = true;
                    break;
                }
            }
        }
        Ok(result)
    }

    fn read_frame(&mut self) -> std::io::Result<Vec<u8>> {
        use std::io::Read;
        let mut tag = [0u8; 1];
        self.stdout.read_exact(&mut tag)?;
        if tag[0] != 0x0a {
            return Err(std::io::Error::other(format!("bad frame tag: {:#04x}", tag[0])));
        }
        let len = self.read_stream_varint()? as usize;
        let mut body = vec![0u8; len];
        self.stdout.read_exact(&mut body)?;
        Ok(body)
    }

    fn read_stream_varint(&mut self) -> std::io::Result<u64> {
        use std::io::Read;
        let (mut value, mut shift) = (0u64, 0u32);
        loop {
            let mut byte = [0u8; 1];
            self.stdout.read_exact(&mut byte)?;
            value |= ((byte[0] & 0x7F) as u64) << shift;
            if byte[0] & 0x80 == 0 {
                return Ok(value);
            }
            shift += 7;
            if shift >= 64 {
                return Err(std::io::Error::other("varint overflow"));
            }
        }
    }

    /// Kills the trace_processor process and waits for it to exit.
    pub fn shutdown(mut self) -> std::io::Result<()> {
        drop(self.stdin);
        drop(self.stdout);
        let _ = self.child.kill();
        self.child.wait().map(|_| ())
    }
}
