use super::{BackpressureMode, BufferLimit, Config, IngestProtocol, StorageConfig, UpstreamMode};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn empty_config_file_uses_defaults() {
    let path = write_temp_config("defaults", "{}\n");
    let config = Config::load(&path).unwrap();

    assert_eq!(config.ingest_addr, "127.0.0.1:7001");
    assert_eq!(config.ingest_protocol, IngestProtocol::Wire);
    assert!(!config.ingest_tls.enable);
    assert!(config.ingest_tls.ca_file.is_none());
    assert!(config.ingest_tls.cert_file.is_none());
    assert!(config.ingest_tls.key_file.is_none());
    assert!(!config.ingest_tls.require_client_cert);
    assert_eq!(config.ingest_limits.max_batch_bytes, 1024 * 1024);
    assert_eq!(config.ingest_limits.max_clients, 32);
    assert_eq!(config.replay_addr, "0.0.0.0:7002");
    assert_eq!(config.replay_max_clients, 32);
    assert_eq!(config.replay_client_timeout_ms, 10_000);
    assert_eq!(config.collector.url, "http://127.0.0.1:4318/v1/logs");
    assert_eq!(config.collector.timeout_ms, 10_000);
    assert!(!config.backpressure.enabled);
    assert_eq!(config.backpressure.mode, BackpressureMode::Disconnect);
    assert_eq!(config.backpressure.max_buffered_records, 16);
    assert!(config.collector.ca_file.is_none());
    assert!(config.collector.cert_file.is_none());
    assert!(config.collector.key_file.is_none());
    assert!(config.collector.server_name.is_none());
    assert!(config.upstream.replay_addr.is_none());
    assert_eq!(config.upstream.mode, UpstreamMode::Keep);
    assert!(config.upstream.state_file.is_none());
    assert_eq!(config.upstream.retry_ms, 1_000);
    assert_eq!(config.upstream.connect_timeout_ms, 5_000);
    assert!(!config.tls.enable);
    assert!(config.tls.ca_file.is_none());
    assert!(config.tls.cert_file.is_none());
    assert!(config.tls.key_file.is_none());
    assert!(!config.tls.require_client_cert);
    assert!(config.tls.server_name.is_none());

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
        "output: file\nfile.path: ./logs\nfile.size: 16\nfile.name: bofh.logjet\ningest.protocol: otlp-grpc\ningest.tls-enable: true\ningest.ca-file: ./ingest-ca.pem\ningest.cert-file: ./ingest.pem\ningest.key-file: ./ingest.key\ningest.require-client-cert: true\ningest.max-batch-bytes: 262144\ningest.max-clients: 7\ncollector.url: https://127.0.0.1:4320/custom\ncollector.timeout-ms: 3210\ncollector.ca-file: ./collector-ca.pem\ncollector.cert-file: ./collector.pem\ncollector.key-file: ./collector.key\ncollector.server-name: collector.internal\nbackpressure.enabled: true\nbackpressure.mode: block\nbackpressure.max-buffered-records: 23\nupstream.replay: 10.0.0.15:7002\nupstream.retry-ms: 222\nupstream.connect-timeout-ms: 333\ntls.enable: true\ntls.ca-file: ./ca.pem\ntls.cert-file: ./node.pem\ntls.key-file: ./node.key\ntls.require-client-cert: true\ntls.server-name: appliance.internal\nreplay.max-clients: 9\nreplay.client-timeout-ms: 444\n",
    );
    let config = Config::load(&path).unwrap();

    assert_eq!(config.ingest_protocol, IngestProtocol::OtlpGrpc);
    assert_eq!(config.collector.timeout_ms, 3210);
    assert_eq!(config.upstream.replay_addr.as_deref(), Some("10.0.0.15:7002"));
    assert_eq!(config.upstream.mode, UpstreamMode::Keep);
    assert!(config.upstream.state_file.is_none());
    assert_eq!(config.upstream.retry_ms, 222);
    assert_eq!(config.upstream.connect_timeout_ms, 333);
    assert!(config.ingest_tls.enable);
    assert_eq!(config.ingest_tls.ca_file.as_deref(), Some(Path::new("./ingest-ca.pem")));
    assert_eq!(config.ingest_tls.cert_file.as_deref(), Some(Path::new("./ingest.pem")));
    assert_eq!(config.ingest_tls.key_file.as_deref(), Some(Path::new("./ingest.key")));
    assert!(config.ingest_tls.require_client_cert);
    assert_eq!(config.ingest_limits.max_batch_bytes, 262_144);
    assert_eq!(config.ingest_limits.max_clients, 7);
    assert_eq!(config.replay_max_clients, 9);
    assert_eq!(config.replay_client_timeout_ms, 444);
    assert!(config.tls.enable);
    assert_eq!(config.tls.ca_file.as_deref(), Some(Path::new("./ca.pem")));
    assert_eq!(config.tls.cert_file.as_deref(), Some(Path::new("./node.pem")));
    assert_eq!(config.tls.key_file.as_deref(), Some(Path::new("./node.key")));
    assert!(config.tls.require_client_cert);
    assert_eq!(config.tls.server_name.as_deref(), Some("appliance.internal"));
    assert_eq!(config.collector.url, "https://127.0.0.1:4320/custom");
    assert!(config.backpressure.enabled);
    assert_eq!(config.backpressure.mode, BackpressureMode::Block);
    assert_eq!(config.backpressure.max_buffered_records, 23);
    assert_eq!(config.collector.ca_file.as_deref(), Some(Path::new("./collector-ca.pem")));
    assert_eq!(config.collector.cert_file.as_deref(), Some(Path::new("./collector.pem")));
    assert_eq!(config.collector.key_file.as_deref(), Some(Path::new("./collector.key")));
    assert_eq!(config.collector.server_name.as_deref(), Some("collector.internal"));

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

#[test]
fn https_collector_fields_parse_without_file_mode() {
    let path = write_temp_config(
        "collector-https",
        "collector.url: https://collector.example:443/v1/logs\ncollector.ca-file: ./ca.pem\ncollector.server-name: collector.example\n",
    );
    let config = Config::load(&path).unwrap();
    assert_eq!(config.collector.url, "https://collector.example:443/v1/logs");
    assert_eq!(config.collector.ca_file.as_deref(), Some(Path::new("./ca.pem")));
    assert_eq!(config.collector.server_name.as_deref(), Some("collector.example"));
    fs::remove_file(path).unwrap();
}

#[test]
fn upstream_mode_drain_parses() {
    let path = write_temp_config(
        "upstream-drain",
        "upstream.mode: drain\nupstream.replay: 127.0.0.1:7002\nupstream.state-file: ./bridge.state\n",
    );
    let config = Config::load(&path).unwrap();
    assert_eq!(config.upstream.mode, UpstreamMode::Drain);
    assert_eq!(config.upstream.state_file.as_deref(), Some(Path::new("./bridge.state")));
    fs::remove_file(path).unwrap();
}

#[test]
fn invalid_upstream_mode_is_rejected() {
    let path = write_temp_config("bad-upstream-mode", "upstream.mode: nope\n");
    let err = Config::load(&path).unwrap_err().to_string();
    assert!(err.contains("invalid upstream mode"));
    fs::remove_file(path).unwrap();
}

#[test]
fn invalid_backpressure_mode_is_rejected() {
    let path = write_temp_config("bad-backpressure-mode", "backpressure.mode: nope\n");
    let err = Config::load(&path).unwrap_err().to_string();
    assert!(err.contains("invalid backpressure mode"));
    fs::remove_file(path).unwrap();
}

#[test]
fn backpressure_mode_block_parses() {
    let path = write_temp_config("backpressure-block", "backpressure.enabled: true\nbackpressure.mode: block\n");
    let config = Config::load(&path).unwrap();
    assert!(config.backpressure.enabled);
    assert_eq!(config.backpressure.mode, BackpressureMode::Block);
    fs::remove_file(path).unwrap();
}

#[test]
fn backpressure_mode_drop_newest_parses() {
    let path = write_temp_config(
        "backpressure-drop-newest",
        "backpressure.enabled: true\nbackpressure.mode: drop-newest\nbackpressure.max-buffered-records: 3\n",
    );
    let config = Config::load(&path).unwrap();
    assert!(config.backpressure.enabled);
    assert_eq!(config.backpressure.mode, BackpressureMode::DropNewest);
    assert_eq!(config.backpressure.max_buffered_records, 3);
    fs::remove_file(path).unwrap();
}

#[test]
fn invalid_ingest_limit_values_are_rejected() {
    let batch_path = write_temp_config("bad-ingest-batch", "ingest.max-batch-bytes: 0\n");
    let batch_err = Config::load(&batch_path).unwrap_err().to_string();
    assert!(batch_err.contains("ingest.max-batch-bytes"));
    fs::remove_file(batch_path).unwrap();

    let clients_path = write_temp_config("bad-ingest-clients", "ingest.max-clients: 0\n");
    let clients_err = Config::load(&clients_path).unwrap_err().to_string();
    assert!(clients_err.contains("ingest.max-clients"));
    fs::remove_file(clients_path).unwrap();

    let replay_path = write_temp_config("bad-replay-clients", "replay.max-clients: 0\n");
    let replay_err = Config::load(&replay_path).unwrap_err().to_string();
    assert!(replay_err.contains("replay.max-clients"));
    fs::remove_file(replay_path).unwrap();

    let replay_timeout_path = write_temp_config("bad-replay-timeout", "replay.client-timeout-ms: 0\n");
    let replay_timeout_err = Config::load(&replay_timeout_path).unwrap_err().to_string();
    assert!(replay_timeout_err.contains("replay.client-timeout-ms"));
    fs::remove_file(replay_timeout_path).unwrap();

    let backpressure_buffer_path = write_temp_config(
        "bad-backpressure-buffer",
        "backpressure.max-buffered-records: 0\n",
    );
    let backpressure_buffer_err = Config::load(&backpressure_buffer_path).unwrap_err().to_string();
    assert!(backpressure_buffer_err.contains("backpressure.max-buffered-records"));
    fs::remove_file(backpressure_buffer_path).unwrap();
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
