use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, BufReader};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use logjet::LogjetReader;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use prost::Message;

#[test]
fn ljx_filters_real_ljd_output_from_mock_emitter() -> io::Result<()> {
    ensure_test_binaries_exist()?;

    let dir = TestDir::new("ljx-cli")?;
    let spool_dir = dir.path().join("spool");
    fs::create_dir_all(&spool_dir)?;
    let ingest_port = free_port()?;
    let spool_path = spool_dir.join("integration.logjet");
    let filtered_literal = dir.path().join("literal.logjet");
    let filtered_regex = dir.path().join("regex.logjet");
    let filtered_seq = dir.path().join("seq.logjet");
    let filtered_stdout = dir.path().join("stdout.logjet");

    let config = dir.write(
        "logjetd.conf",
        &format!(
            "output: file\nfile.path: {}\nfile.size: 1024\nfile.name: integration.logjet\ningest.protocol: otlp-http\ningest.listen: 127.0.0.1:{ingest_port}\n",
            spool_dir.display()
        ),
    )?;

    eprintln!("starting ljd");
    let _daemon = ChildGuard::spawn({
        let mut cmd = Command::new(ljd_bin());
        cmd.arg("--config").arg(&config).arg("serve");
        cmd
    })
    .map_err(|err| io::Error::other(format!("failed to start ljd: {err}")))?;
    eprintln!("waiting for ingest tcp");
    wait_for_tcp(&format!("127.0.0.1:{ingest_port}"), Duration::from_secs(5))
        .map_err(|err| io::Error::other(format!("failed waiting for ingest tcp: {err}")))?;

    for message in ["java.crap.failed", "java.alpha.bs", "ERROR boom", "eRrOr splash", "banana"] {
        eprintln!("emitting {message}");
        run_emitter(ingest_port, message)?;
    }

    eprintln!("waiting for spool file");
    wait_until(Duration::from_secs(5), || Ok(spool_path.exists() && fs::metadata(&spool_path)?.len() > 0))
        .map_err(|err| io::Error::other(format!("failed waiting for spool file: {err}")))?;

    let records = read_logjet_records(&spool_path)?;
    assert_eq!(records.len(), 5);
    let seq_min = records[1].seq;
    let seq_max = records[3].seq;
    let ts_min = records[1].ts_unix_ns;
    let ts_max = records[3].ts_unix_ns;

    eprintln!("running ljx assertions");
    assert_eq!(run_ljx_count(&spool_path, &[])?, "5");
    assert_eq!(run_ljx_count(&spool_path, &["--type", "logs"])?, "5");
    assert_eq!(run_ljx_count(&spool_path, &["-F", "java.crap.failed"])?, "1");
    assert_eq!(run_ljx_count(&spool_path, &["-e", r"java\..*\.bs"])?, "1");
    assert_eq!(run_ljx_count(&spool_path, &["-F", "error", "-i"])?, "2");
    assert_eq!(run_ljx_count(&spool_path, &["--seq-min", &seq_min.to_string(), "--seq-max", &seq_max.to_string(),],)?, "3");
    assert_eq!(run_ljx_count(&spool_path, &["--ts-min", &ts_min.to_string(), "--ts-max", &ts_max.to_string(),],)?, "3");

    run_ljx_filter(&spool_path, &filtered_literal, &["-F", "java.crap.failed"])?;
    assert_eq!(read_logjet_messages(&filtered_literal)?, vec!["java.crap.failed".to_string()]);

    run_ljx_filter(&spool_path, &filtered_regex, &["-e", "error|panic", "-i"])?;
    assert_eq!(read_logjet_messages(&filtered_regex)?, vec!["ERROR boom".to_string(), "eRrOr splash".to_string()]);

    run_ljx_filter(&spool_path, &filtered_seq, &["--seq-min", &seq_min.to_string(), "--seq-max", &seq_max.to_string()])?;
    assert_eq!(read_logjet_messages(&filtered_seq)?, vec!["java.alpha.bs".to_string(), "ERROR boom".to_string(), "eRrOr splash".to_string()]);

    let stdout_output = run_ljx(["filter".as_ref(), spool_path.as_os_str(), "-o".as_ref(), "-".as_ref()], &["-F", "error", "-i"])?;
    if !stdout_output.status.success() {
        return Err(io::Error::other(format!("ljx stdout filter failed: {}", String::from_utf8_lossy(&stdout_output.stderr))));
    }
    fs::write(&filtered_stdout, &stdout_output.stdout)?;
    assert_eq!(read_logjet_messages(&filtered_stdout)?, vec!["ERROR boom".to_string(), "eRrOr splash".to_string()]);

    let invalid = run_ljx(["count".as_ref(), spool_path.as_os_str()], &["-F", "error", "-e", "panic"])?;
    assert!(!invalid.status.success());
    assert!(
        String::from_utf8_lossy(&invalid.stderr).contains("cannot be used with")
            || String::from_utf8_lossy(&invalid.stderr).contains("choose either")
    );

    Ok(())
}

fn ensure_test_binaries_exist() -> io::Result<()> {
    for path in [ljd_bin(), ljx_bin(), emitter_bin()] {
        if !path.is_file() {
            return Err(io::Error::other(format!(
                "missing test binary {}. build it first with: cargo build -p ljd -p ljx -p otlp-demo --bin otlp-bofh-emitter",
                path.display()
            )));
        }
    }
    Ok(())
}

fn run_emitter(ingest_port: u16, message: &str) -> io::Result<()> {
    let status = Command::new(emitter_bin())
        .arg(format!("127.0.0.1:{ingest_port}"))
        .arg("--once")
        .arg("--service-name")
        .arg("ljx-it")
        .arg("--message")
        .arg(message)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|err| io::Error::other(format!("failed to start emitter: {err}")))?;
    if status.success() { Ok(()) } else { Err(io::Error::other(format!("emitter failed for message: {message}"))) }
}

fn run_ljx_count(input: &Path, extra_args: &[&str]) -> io::Result<String> {
    let output = run_ljx(["count".as_ref(), input.as_os_str()], extra_args)?;
    if !output.status.success() {
        return Err(io::Error::other(format!("ljx count failed: {}", String::from_utf8_lossy(&output.stderr))));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn run_ljx_filter(input: &Path, output: &Path, extra_args: &[&str]) -> io::Result<()> {
    let output = run_ljx(["filter".as_ref(), input.as_os_str(), "-o".as_ref(), output.as_os_str()], extra_args)?;
    if output.status.success() { Ok(()) } else { Err(io::Error::other(format!("ljx filter failed: {}", String::from_utf8_lossy(&output.stderr)))) }
}

fn run_ljx<const N: usize>(prefix_args: [&OsStr; N], extra_args: &[&str]) -> io::Result<Output> {
    let mut command = Command::new(ljx_bin());
    command.args(prefix_args);
    command.args(extra_args);
    command.output().map_err(|err| io::Error::other(format!("failed to start ljx: {err}")))
}

fn read_logjet_messages(path: &Path) -> io::Result<Vec<String>> {
    Ok(read_logjet_records(path)?.into_iter().map(|record| record.message).collect())
}

fn read_logjet_records(path: &Path) -> io::Result<Vec<DecodedRecord>> {
    let file = File::open(path)?;
    let mut reader = LogjetReader::new(BufReader::new(file));
    let mut records = Vec::new();
    while let Some(record) = reader.next_record().map_err(io::Error::other)? {
        for message in decode_payload_messages(&record.payload)? {
            records.push(DecodedRecord { seq: record.seq, ts_unix_ns: record.ts_unix_ns, message });
        }
    }
    Ok(records)
}

fn decode_payload_messages(payload: &[u8]) -> io::Result<Vec<String>> {
    let batch = ExportLogsServiceRequest::decode(payload).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?;
    let mut messages = Vec::new();
    for resource_logs in batch.resource_logs {
        for scope_logs in resource_logs.scope_logs {
            for record in scope_logs.log_records {
                if let Some(body) = record.body
                    && let Some(Value::StringValue(message)) = body.value
                {
                    messages.push(message);
                }
            }
        }
    }
    Ok(messages)
}

fn target_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target"))
}

struct DecodedRecord {
    seq: u64,
    ts_unix_ns: u64,
    message: String,
}

fn ljd_bin() -> PathBuf {
    target_dir().join("debug").join(binary_name("ljd"))
}

fn ljx_bin() -> PathBuf {
    target_dir().join("debug").join(binary_name("ljx"))
}

fn emitter_bin() -> PathBuf {
    target_dir().join("debug").join(binary_name("otlp-bofh-emitter"))
}

fn binary_name(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_string() }
}

struct TestDir {
    path: PathBuf,
}

impl TestDir {
    fn new(label: &str) -> io::Result<Self> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("logjet-ljx-it-{label}-{nanos}-{}", std::process::id()));
        fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn write(&self, name: &str, body: &str) -> io::Result<PathBuf> {
        let path = self.path.join(name);
        fs::write(&path, body)?;
        Ok(path)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct ChildGuard {
    child: Child,
}

impl ChildGuard {
    fn spawn(mut command: Command) -> io::Result<Self> {
        let child = command.stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
        Ok(Self { child })
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> io::Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

fn wait_for_tcp(addr: &str, timeout: Duration) -> io::Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        match TcpStream::connect(addr) {
            Ok(stream) => {
                let _ = stream.shutdown(Shutdown::Both);
                return Ok(());
            }
            Err(err) if Instant::now() < deadline => {
                if err.kind() != io::ErrorKind::ConnectionRefused
                    && err.kind() != io::ErrorKind::TimedOut
                    && err.kind() != io::ErrorKind::AddrNotAvailable
                {
                    return Err(err);
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err(err) => return Err(err),
        }
    }
}

fn wait_until<F>(timeout: Duration, mut predicate: F) -> io::Result<()>
where
    F: FnMut() -> io::Result<bool>,
{
    let deadline = Instant::now() + timeout;
    loop {
        if predicate()? {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "timed out waiting for condition"));
        }
        thread::sleep(Duration::from_millis(25));
    }
}
