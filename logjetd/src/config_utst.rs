use super::{BufferLimit, Config, IngestProtocol, StorageConfig};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn empty_config_file_uses_defaults() {
    let path = write_temp_config("defaults", "{}\n");
    let config = Config::load(&path).unwrap();

    assert_eq!(config.ingest_addr, "127.0.0.1:7001");
    assert_eq!(config.ingest_protocol, IngestProtocol::Wire);
    assert_eq!(config.replay_addr, "0.0.0.0:7002");
    assert_eq!(config.poll_interval_ms, 250);
    assert_eq!(config.collector.url, "http://127.0.0.1:4318/v1/logs");
    assert_eq!(config.collector.timeout_ms, 10_000);
    assert!(config.upstream.replay_addr.is_none());
    assert_eq!(config.upstream.retry_ms, 1_000);
    assert_eq!(config.upstream.connect_timeout_ms, 5_000);

    match config.storage {
        StorageConfig::Buffer(buffer) => assert_eq!(buffer.limit, BufferLimit::Bytes(100 * 1024)),
        StorageConfig::File(_) => panic!("expected buffer storage by default"),
    }

    fs::remove_file(path).unwrap();
}

#[test]
fn buffer_size_and_messages_conflict() {
    let path = write_temp_config(
        "buffer-conflict",
        "output: buffer\nbuffer.size: 10\nbuffer.messages: 5\n",
    );
    let err = Config::load(&path).unwrap_err().to_string();
    assert!(err.contains("buffer.size and buffer.messages conflict"));
    fs::remove_file(path).unwrap();
}

#[test]
fn file_mode_and_collector_settings_parse() {
    let path = write_temp_config(
        "file-mode",
        "output: file\nfile.path: ./logs\nfile.size: 16\nfile.name: bofh.logjet\ningest.protocol: otlp-grpc\ncollector.url: http://127.0.0.1:4320/custom\ncollector.timeout-ms: 3210\nupstream.replay: 10.0.0.15:7002\nupstream.retry-ms: 222\nupstream.connect-timeout-ms: 333\n",
    );
    let config = Config::load(&path).unwrap();

    assert_eq!(config.ingest_protocol, IngestProtocol::OtlpGrpc);
    assert_eq!(config.collector.url, "http://127.0.0.1:4320/custom");
    assert_eq!(config.collector.timeout_ms, 3210);
    assert_eq!(config.upstream.replay_addr.as_deref(), Some("10.0.0.15:7002"));
    assert_eq!(config.upstream.retry_ms, 222);
    assert_eq!(config.upstream.connect_timeout_ms, 333);

    match config.storage {
        StorageConfig::File(file) => {
            assert_eq!(file.dir, PathBuf::from("./logs"));
            assert_eq!(file.name, "bofh.logjet");
            assert_eq!(file.segment_size_bytes, 16 * 1024);
        }
        StorageConfig::Buffer(_) => panic!("expected file storage"),
    }

    fs::remove_file(path).unwrap();
}

#[test]
fn invalid_ingest_protocol_is_rejected() {
    let path = write_temp_config("bad-protocol", "ingest.protocol: nope\n");
    let err = Config::load(&path).unwrap_err().to_string();
    assert!(err.contains("invalid ingest protocol"));
    fs::remove_file(path).unwrap();
}

fn write_temp_config(label: &str, body: &str) -> PathBuf {
    let path = unique_temp_path(label);
    fs::write(&path, body).unwrap();
    path
}

fn unique_temp_path(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("logjetd-{label}-{nanos}-{}.yaml", std::process::id()))
}
