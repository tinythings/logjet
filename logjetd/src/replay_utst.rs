use super::{
    BridgeState, CollectorEndpoint, CollectorTransport, EnqueueOutcome, ExportTask, enqueue_export_task, parse_bridge_state, read_bridge_state,
    reconcile_bridge_state, write_bridge_state,
};
use crate::config::{BackpressureMode, CollectorConfig, UpstreamMode};
use crate::protocol::ReplayHello;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn host_port_defaults_to_v1_logs() {
    let endpoint = CollectorEndpoint::parse("127.0.0.1:4318").unwrap();
    assert_eq!(endpoint.authority, "127.0.0.1:4318");
    assert_eq!(endpoint.path, "/v1/logs");
}

#[test]
fn http_url_with_custom_path_is_preserved() {
    let endpoint = CollectorEndpoint::parse("http://127.0.0.1:4318/custom/path").unwrap();
    assert_eq!(endpoint.authority, "127.0.0.1:4318");
    assert_eq!(endpoint.path, "/custom/path");
}

#[test]
fn http_url_without_leading_slash_is_normalised() {
    let endpoint = CollectorEndpoint::parse("http://127.0.0.1:4318/custom").unwrap();
    assert_eq!(endpoint.path, "/custom");
}

#[test]
fn https_url_is_supported() {
    let endpoint = CollectorEndpoint::parse("https://127.0.0.1:4318/v1/logs").unwrap();
    assert_eq!(endpoint.authority, "127.0.0.1:4318");
    assert_eq!(endpoint.path, "/v1/logs");
    assert!(endpoint.tls);
}

#[test]
fn missing_authority_is_rejected() {
    let err = CollectorEndpoint::parse("http:///v1/logs").err().unwrap().to_string();
    assert!(err.contains("missing host:port"));
}

#[test]
fn bridge_state_round_trip() {
    let path = unique_temp_path("bridge-state");
    assert_eq!(read_bridge_state(Some(&path)).unwrap(), BridgeState { stream_id: None, last_seq: 0 });
    write_bridge_state(Some(&path), &BridgeState { stream_id: Some(42), last_seq: 77 }).unwrap();
    assert_eq!(read_bridge_state(Some(&path)).unwrap(), BridgeState { stream_id: Some(42), last_seq: 77 });
    fs::remove_file(path).unwrap();
}

#[test]
fn invalid_bridge_state_is_rejected() {
    let path = unique_temp_path("bridge-state-invalid");
    fs::write(&path, "not-a-number").unwrap();
    let err = read_bridge_state(Some(&path)).err().unwrap();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    fs::remove_file(path).unwrap();
}

#[test]
fn legacy_numeric_bridge_state_still_parses() {
    let state = parse_bridge_state("123\n").unwrap();
    assert_eq!(state, BridgeState { stream_id: None, last_seq: 123 });
}

#[test]
fn bridge_state_resets_on_stream_id_change() {
    let mut state = BridgeState { stream_id: Some(11), last_seq: 99 };
    reconcile_bridge_state("127.0.0.1:7002", &mut state, &ReplayHello { stream_id: 22, first_seq: 1, last_seq: 5 }).unwrap();
    assert_eq!(state, BridgeState { stream_id: Some(22), last_seq: 0 });
}

#[test]
fn bridge_state_resets_when_legacy_saved_seq_is_above_upstream_last_seq() {
    let mut state = BridgeState { stream_id: None, last_seq: 99 };
    reconcile_bridge_state("127.0.0.1:7002", &mut state, &ReplayHello { stream_id: 55, first_seq: 1, last_seq: 5 }).unwrap();
    assert_eq!(state, BridgeState { stream_id: Some(55), last_seq: 0 });
}

#[test]
fn disconnect_mode_errors_when_export_queue_is_full() {
    let transport = test_collector_transport(BackpressureMode::Disconnect, 1);
    let (task_tx, _task_rx) = mpsc::sync_channel(1);
    task_tx.send(ExportTask { seq: 1, payload: vec![1] }).unwrap();

    let err = enqueue_export_task(&task_tx, &transport, ExportTask { seq: 2, payload: vec![2] }).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
}

#[test]
fn drop_newest_mode_reports_drop_when_export_queue_is_full() {
    let transport = test_collector_transport(BackpressureMode::DropNewest, 1);
    let (task_tx, _task_rx) = mpsc::sync_channel(1);
    task_tx.send(ExportTask { seq: 1, payload: vec![1] }).unwrap();

    let outcome = enqueue_export_task(&task_tx, &transport, ExportTask { seq: 2, payload: vec![2] }).unwrap();
    assert_eq!(outcome, EnqueueOutcome::DroppedNewest);
}

fn test_collector_transport(mode: BackpressureMode, max_buffered_records: usize) -> CollectorTransport {
    CollectorTransport {
        endpoint: CollectorEndpoint { authority: "127.0.0.1:4318".to_string(), path: "/v1/logs".to_string(), tls: false },
        timeout: std::time::Duration::from_millis(1000),
        backpressure_enabled: true,
        backpressure_mode: mode,
        max_buffered_records,
        tls_client: None::<Arc<rustls::ClientConfig>>,
        collector: CollectorConfig {
            url: "http://127.0.0.1:4318/v1/logs".to_string(),
            timeout_ms: 1000,
            ca_file: None,
            cert_file: None,
            key_file: None,
            server_name: None,
        },
        upstream_mode: UpstreamMode::Keep,
    }
}

fn unique_temp_path(label: &str) -> PathBuf {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    std::env::temp_dir().join(format!("ljd-{label}-{nanos}-{}.state", std::process::id()))
}
