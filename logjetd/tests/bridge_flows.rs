mod common;

use std::fs;
use std::io;
use std::net::TcpStream;
use std::thread;
use std::time::Duration;

use common::{
    ChildGuard, MockCollector, MockGrpcCollector, ReservedPort, TestDir, connect_replay_client, ensure_rustls_provider, free_port, ljd_command, post_otlp_http,
    read_replay_message, replay_messages, reserve_port, wait_for_tcp, wait_until, write_fake_grpc_tls_files,
};

fn http_collector(port: ReservedPort) -> io::Result<MockCollector> {
    MockCollector::start_with_listener(port.into_listener(), Duration::ZERO)
}

fn reserved_port_addr(port: &ReservedPort) -> u16 {
    port.port()
}

#[test]
fn bridge_keep_forwards_backlog_in_order() -> io::Result<()> {
    let dir = TestDir::new("bridge-keep")?;
    let ingest_port = free_port()?;
    let replay_port = free_port()?;
    let collector_port = reserve_port()?;
    let collector_addr = collector_port.port();

    let appliance_config = dir.write(
        "appliance.conf",
        &format!(
            "output: buffer\nbuffer.messages: 64\ningest.protocol: otlp-http\ningest.listen: 127.0.0.1:{ingest_port}\nreplay.listen: 127.0.0.1:{replay_port}\n"
        ),
    )?;
    let bridge_config = dir.write(
        "bridge.conf",
        &format!("collector.url: http://127.0.0.1:{collector_addr}/v1/logs\nupstream.replay: 127.0.0.1:{replay_port}\nupstream.mode: keep\n"),
    )?;

    let _appliance = ChildGuard::spawn({
        let mut cmd = ljd_command();
        cmd.arg("--config").arg(&appliance_config).arg("serve");
        cmd
    })?;
    wait_for_tcp(&format!("127.0.0.1:{ingest_port}"), Duration::from_secs(5))?;
    wait_for_tcp(&format!("127.0.0.1:{replay_port}"), Duration::from_secs(5))?;

    for message in ["KEEP 001", "KEEP 002", "KEEP 003"] {
        post_otlp_http(&format!("127.0.0.1:{ingest_port}"), "bridge-keep", message)?;
    }

    let collector = http_collector(collector_port)?;
    let _bridge = ChildGuard::spawn({
        let mut cmd = ljd_command();
        cmd.arg("--config").arg(&bridge_config).arg("bridge");
        cmd
    })?;

    wait_until(Duration::from_secs(5), || Ok(collector.messages().len() >= 3))?;
    assert_eq!(collector.messages(), vec!["KEEP 001".to_string(), "KEEP 002".to_string(), "KEEP 003".to_string()]);

    let retained = replay_messages(&format!("127.0.0.1:{replay_port}"), 0, 3)?;
    assert_eq!(retained, vec!["KEEP 001".to_string(), "KEEP 002".to_string(), "KEEP 003".to_string()]);

    Ok(())
}

#[test]
fn bridge_drain_consumes_upstream_records() -> io::Result<()> {
    let dir = TestDir::new("bridge-drain")?;
    let ingest_port = free_port()?;
    let replay_port = free_port()?;
    let collector_port = reserve_port()?;

    let appliance_config = dir.write(
        "appliance.conf",
        &format!(
            "output: buffer\nbuffer.messages: 64\ningest.protocol: otlp-http\ningest.listen: 127.0.0.1:{ingest_port}\nreplay.listen: 127.0.0.1:{replay_port}\n"
        ),
    )?;
    let bridge_config = dir.write(
        "bridge.conf",
        &format!(
            "collector.url: http://127.0.0.1:{}/v1/logs\nupstream.replay: 127.0.0.1:{replay_port}\nupstream.mode: drain\n",
            reserved_port_addr(&collector_port)
        ),
    )?;

    let _appliance = ChildGuard::spawn({
        let mut cmd = ljd_command();
        cmd.arg("--config").arg(&appliance_config).arg("serve");
        cmd
    })?;
    wait_for_tcp(&format!("127.0.0.1:{ingest_port}"), Duration::from_secs(5))?;
    wait_for_tcp(&format!("127.0.0.1:{replay_port}"), Duration::from_secs(5))?;

    for message in ["DRAIN 001", "DRAIN 002", "DRAIN 003"] {
        post_otlp_http(&format!("127.0.0.1:{ingest_port}"), "bridge-drain", message)?;
    }

    let collector = http_collector(collector_port)?;
    let _bridge = ChildGuard::spawn({
        let mut cmd = ljd_command();
        cmd.arg("--config").arg(&bridge_config).arg("bridge");
        cmd
    })?;

    wait_until(Duration::from_secs(5), || Ok(collector.messages().len() >= 3))?;
    assert_eq!(collector.messages(), vec!["DRAIN 001".to_string(), "DRAIN 002".to_string(), "DRAIN 003".to_string()]);

    wait_until(Duration::from_secs(5), || Ok(replay_messages(&format!("127.0.0.1:{replay_port}"), 0, 1)?.is_empty()))?;

    Ok(())
}

#[test]
fn bridge_keep_forwards_backlog_over_grpc() -> io::Result<()> {
    let dir = TestDir::new("bridge-keep-grpc")?;
    let ingest_port = free_port()?;
    let replay_port = free_port()?;
    let collector_port = free_port()?;

    let appliance_config = dir.write(
        "appliance.conf",
        &format!(
            "output: buffer\nbuffer.messages: 64\ningest.protocol: otlp-http\ningest.listen: 127.0.0.1:{ingest_port}\nreplay.listen: 127.0.0.1:{replay_port}\n"
        ),
    )?;
    let bridge_config = dir.write(
        "bridge.conf",
        &format!("collector.url: grpc://127.0.0.1:{collector_port}\nupstream.replay: 127.0.0.1:{replay_port}\nupstream.mode: keep\n"),
    )?;

    let _appliance = ChildGuard::spawn({
        let mut cmd = ljd_command();
        cmd.arg("--config").arg(&appliance_config).arg("serve");
        cmd
    })?;
    wait_for_tcp(&format!("127.0.0.1:{ingest_port}"), Duration::from_secs(5))?;
    wait_for_tcp(&format!("127.0.0.1:{replay_port}"), Duration::from_secs(5))?;

    for message in ["GRPC 001", "GRPC 002", "GRPC 003"] {
        post_otlp_http(&format!("127.0.0.1:{ingest_port}"), "bridge-grpc", message)?;
    }

    let collector = MockGrpcCollector::start(collector_port)?;
    let _bridge = ChildGuard::spawn({
        let mut cmd = ljd_command();
        cmd.arg("--config").arg(&bridge_config).arg("bridge");
        cmd
    })?;

    wait_until(Duration::from_secs(5), || Ok(collector.messages().len() >= 3))?;
    assert_eq!(collector.messages(), vec!["GRPC 001".to_string(), "GRPC 002".to_string(), "GRPC 003".to_string()]);

    Ok(())
}

#[test]
fn bridge_keep_fans_out_to_http_and_grpc() -> io::Result<()> {
    let dir = TestDir::new("bridge-fanout")?;
    let ingest_port = free_port()?;
    let replay_port = free_port()?;
    let http_port = free_port()?;
    let grpc_port = free_port()?;

    let appliance_config = dir.write(
        "appliance.conf",
        &format!(
            "output: buffer\nbuffer.messages: 64\ningest.protocol: otlp-http\ningest.listen: 127.0.0.1:{ingest_port}\nreplay.listen: 127.0.0.1:{replay_port}\n"
        ),
    )?;
    let bridge_config = dir.write(
        "bridge.conf",
        &format!(
            "collector.url:\n  - http://127.0.0.1:{http_port}/v1/logs\n  - grpc://127.0.0.1:{grpc_port}\nupstream.replay: 127.0.0.1:{replay_port}\nupstream.mode: keep\n"
        ),
    )?;

    let _appliance = ChildGuard::spawn({
        let mut cmd = ljd_command();
        cmd.arg("--config").arg(&appliance_config).arg("serve");
        cmd
    })?;
    wait_for_tcp(&format!("127.0.0.1:{ingest_port}"), Duration::from_secs(5))?;
    wait_for_tcp(&format!("127.0.0.1:{replay_port}"), Duration::from_secs(5))?;

    for message in ["FANOUT 001", "FANOUT 002", "FANOUT 003"] {
        post_otlp_http(&format!("127.0.0.1:{ingest_port}"), "bridge-fanout", message)?;
    }

    let http = MockCollector::start(http_port)?;
    let grpc = MockGrpcCollector::start(grpc_port)?;
    wait_for_tcp(&format!("127.0.0.1:{http_port}"), Duration::from_secs(5))?;
    wait_for_tcp(&format!("127.0.0.1:{grpc_port}"), Duration::from_secs(5))?;
    let _bridge = ChildGuard::spawn({
        let mut cmd = ljd_command();
        cmd.arg("--config").arg(&bridge_config).arg("bridge");
        cmd
    })?;

    wait_until(Duration::from_secs(5), || Ok(http.messages().len() >= 3 && grpc.messages().len() >= 3))?;
    assert_eq!(http.messages(), vec!["FANOUT 001".to_string(), "FANOUT 002".to_string(), "FANOUT 003".to_string()]);
    assert_eq!(grpc.messages(), vec!["FANOUT 001".to_string(), "FANOUT 002".to_string(), "FANOUT 003".to_string()]);

    Ok(())
}

#[test]
fn bridge_keep_forwards_backlog_over_grpcs() -> io::Result<()> {
    ensure_rustls_provider();
    let dir = TestDir::new("bridge-keep-grpcs")?;
    let ingest_port = free_port()?;
    let replay_port = free_port()?;
    let collector_port = free_port()?;
    let tls = write_fake_grpc_tls_files(&dir)?;

    let appliance_config = dir.write(
        "appliance.conf",
        &format!(
            "output: buffer\nbuffer.messages: 64\ningest.protocol: otlp-http\ningest.listen: 127.0.0.1:{ingest_port}\nreplay.listen: 127.0.0.1:{replay_port}\n"
        ),
    )?;
    let bridge_config = dir.write(
        "bridge.conf",
        &format!(
            "collector.url: grpcs://127.0.0.1:{collector_port}\ncollector.ca-file: {}\ncollector.server-name: collector.test.invalid\nupstream.replay: 127.0.0.1:{replay_port}\nupstream.mode: keep\n",
            tls.ca.display()
        ),
    )?;

    let _appliance = ChildGuard::spawn({
        let mut cmd = ljd_command();
        cmd.arg("--config").arg(&appliance_config).arg("serve");
        cmd
    })?;
    wait_for_tcp(&format!("127.0.0.1:{ingest_port}"), Duration::from_secs(5))?;
    wait_for_tcp(&format!("127.0.0.1:{replay_port}"), Duration::from_secs(5))?;

    for message in ["JEDI 001", "JEDI 002", "JEDI 003"] {
        post_otlp_http(&format!("127.0.0.1:{ingest_port}"), "bridge-jedi", message)?;
    }

    let collector = MockGrpcCollector::start_tls(collector_port, &tls)?;
    let _bridge = ChildGuard::spawn({
        let mut cmd = ljd_command();
        cmd.arg("--config").arg(&bridge_config).arg("bridge");
        cmd
    })?;

    wait_until(Duration::from_secs(5), || Ok(collector.messages().len() >= 3))?;
    assert_eq!(collector.messages(), vec!["JEDI 001".to_string(), "JEDI 002".to_string(), "JEDI 003".to_string()]);

    Ok(())
}

#[test]
fn bridge_keep_forwards_backlog_over_grpcs_mtls() -> io::Result<()> {
    ensure_rustls_provider();
    let dir = TestDir::new("bridge-keep-grpcs-mtls")?;
    let ingest_port = free_port()?;
    let replay_port = free_port()?;
    let collector_port = free_port()?;
    let tls = write_fake_grpc_tls_files(&dir)?;

    let appliance_config = dir.write(
        "appliance.conf",
        &format!(
            "output: buffer\nbuffer.messages: 64\ningest.protocol: otlp-http\ningest.listen: 127.0.0.1:{ingest_port}\nreplay.listen: 127.0.0.1:{replay_port}\n"
        ),
    )?;
    let bridge_config = dir.write(
        "bridge.conf",
        &format!(
            "collector.url: grpcs://127.0.0.1:{collector_port}\ncollector.ca-file: {}\ncollector.cert-file: {}\ncollector.key-file: {}\ncollector.server-name: collector.test.invalid\nupstream.replay: 127.0.0.1:{replay_port}\nupstream.mode: keep\n",
            tls.ca.display(),
            tls.client_cert.display(),
            tls.client_key.display()
        ),
    )?;

    let _appliance = ChildGuard::spawn({
        let mut cmd = ljd_command();
        cmd.arg("--config").arg(&appliance_config).arg("serve");
        cmd
    })?;
    wait_for_tcp(&format!("127.0.0.1:{ingest_port}"), Duration::from_secs(5))?;
    wait_for_tcp(&format!("127.0.0.1:{replay_port}"), Duration::from_secs(5))?;

    for message in ["SITH 001", "SITH 002", "SITH 003"] {
        post_otlp_http(&format!("127.0.0.1:{ingest_port}"), "bridge-sith", message)?;
    }

    let collector = MockGrpcCollector::start_mtls(collector_port, &tls)?;
    let _bridge = ChildGuard::spawn({
        let mut cmd = ljd_command();
        cmd.arg("--config").arg(&bridge_config).arg("bridge");
        cmd
    })?;

    wait_until(Duration::from_secs(5), || Ok(collector.messages().len() >= 3))?;
    assert_eq!(collector.messages(), vec!["SITH 001".to_string(), "SITH 002".to_string(), "SITH 003".to_string()]);

    Ok(())
}

#[test]
fn bridge_keep_rejects_grpcs_with_bad_ca() -> io::Result<()> {
    let dir = TestDir::new("bridge-keep-grpcs-bad-ca")?;
    let other = TestDir::new("bridge-keep-grpcs-bad-ca-other")?;
    let ingest_port = free_port()?;
    let replay_port = free_port()?;
    let collector_port = free_port()?;
    let tls = write_fake_grpc_tls_files(&dir)?;
    let wrong_tls = write_fake_grpc_tls_files(&other)?;

    let appliance_config = dir.write(
        "appliance.conf",
        &format!(
            "output: buffer\nbuffer.messages: 64\ningest.protocol: otlp-http\ningest.listen: 127.0.0.1:{ingest_port}\nreplay.listen: 127.0.0.1:{replay_port}\n"
        ),
    )?;
    let bridge_config = dir.write(
        "bridge.conf",
        &format!(
            "collector.url: grpcs://127.0.0.1:{collector_port}\ncollector.ca-file: {}\ncollector.server-name: collector.test.invalid\nupstream.replay: 127.0.0.1:{replay_port}\nupstream.mode: keep\n",
            wrong_tls.ca.display()
        ),
    )?;

    let _appliance = ChildGuard::spawn({
        let mut cmd = ljd_command();
        cmd.arg("--config").arg(&appliance_config).arg("serve");
        cmd
    })?;
    wait_for_tcp(&format!("127.0.0.1:{ingest_port}"), Duration::from_secs(5))?;
    wait_for_tcp(&format!("127.0.0.1:{replay_port}"), Duration::from_secs(5))?;

    post_otlp_http(&format!("127.0.0.1:{ingest_port}"), "bridge-bad-ca", "BAD CA 001")?;

    let collector = MockGrpcCollector::start_tls(collector_port, &tls)?;
    let _bridge = ChildGuard::spawn({
        let mut cmd = ljd_command();
        cmd.arg("--config").arg(&bridge_config).arg("bridge");
        cmd
    })?;

    thread::sleep(Duration::from_millis(750));
    assert!(collector.messages().is_empty());

    Ok(())
}

#[test]
fn bridge_keep_rejects_grpcs_with_wrong_server_name() -> io::Result<()> {
    let dir = TestDir::new("bridge-keep-grpcs-wrong-name")?;
    let ingest_port = free_port()?;
    let replay_port = free_port()?;
    let collector_port = free_port()?;
    let tls = write_fake_grpc_tls_files(&dir)?;

    let appliance_config = dir.write(
        "appliance.conf",
        &format!(
            "output: buffer\nbuffer.messages: 64\ningest.protocol: otlp-http\ningest.listen: 127.0.0.1:{ingest_port}\nreplay.listen: 127.0.0.1:{replay_port}\n"
        ),
    )?;
    let bridge_config = dir.write(
        "bridge.conf",
        &format!(
            "collector.url: grpcs://127.0.0.1:{collector_port}\ncollector.ca-file: {}\ncollector.server-name: wrong.test.invalid\nupstream.replay: 127.0.0.1:{replay_port}\nupstream.mode: keep\n",
            tls.ca.display()
        ),
    )?;

    let _appliance = ChildGuard::spawn({
        let mut cmd = ljd_command();
        cmd.arg("--config").arg(&appliance_config).arg("serve");
        cmd
    })?;
    wait_for_tcp(&format!("127.0.0.1:{ingest_port}"), Duration::from_secs(5))?;
    wait_for_tcp(&format!("127.0.0.1:{replay_port}"), Duration::from_secs(5))?;

    post_otlp_http(&format!("127.0.0.1:{ingest_port}"), "bridge-wrong-name", "WRONG NAME 001")?;

    let collector = MockGrpcCollector::start_tls(collector_port, &tls)?;
    let _bridge = ChildGuard::spawn({
        let mut cmd = ljd_command();
        cmd.arg("--config").arg(&bridge_config).arg("bridge");
        cmd
    })?;

    thread::sleep(Duration::from_millis(750));
    assert!(collector.messages().is_empty());

    Ok(())
}

#[test]
fn bridge_resume_state_survives_restart() -> io::Result<()> {
    let dir = TestDir::new("bridge-resume")?;
    let ingest_port = free_port()?;
    let replay_port = free_port()?;
    let collector_port = free_port()?;
    let state_path = dir.path().join("bridge.state");

    let appliance_config = dir.write(
        "appliance.conf",
        &format!(
            "output: buffer\nbuffer.messages: 64\ningest.protocol: otlp-http\ningest.listen: 127.0.0.1:{ingest_port}\nreplay.listen: 127.0.0.1:{replay_port}\n"
        ),
    )?;
    let bridge_config = dir.write(
        "bridge.conf",
        &format!(
            "collector.url: http://127.0.0.1:{collector_port}/v1/logs\nupstream.replay: 127.0.0.1:{replay_port}\nupstream.mode: keep\nupstream.state-file: {}\n",
            state_path.display()
        ),
    )?;

    let _appliance = ChildGuard::spawn({
        let mut cmd = ljd_command();
        cmd.arg("--config").arg(&appliance_config).arg("serve");
        cmd
    })?;
    wait_for_tcp(&format!("127.0.0.1:{ingest_port}"), Duration::from_secs(5))?;
    wait_for_tcp(&format!("127.0.0.1:{replay_port}"), Duration::from_secs(5))?;

    let collector = MockCollector::start(collector_port)?;

    for message in ["RESUME 001", "RESUME 002", "RESUME 003"] {
        post_otlp_http(&format!("127.0.0.1:{ingest_port}"), "bridge-resume", message)?;
    }

    {
        let _bridge = ChildGuard::spawn({
            let mut cmd = ljd_command();
            cmd.arg("--config").arg(&bridge_config).arg("bridge");
            cmd
        })?;
        wait_until(Duration::from_secs(5), || Ok(collector.messages().len() >= 3))?;
        // Let the bridge commit state after the collector accepted all 3.
        // The mock increments count before sending HTTP 200, so the bridge
        // hasn't written its state file yet when wait_until returns.
        thread::sleep(Duration::from_millis(100));
    }

    for message in ["RESUME 004", "RESUME 005", "RESUME 006"] {
        post_otlp_http(&format!("127.0.0.1:{ingest_port}"), "bridge-resume", message)?;
    }

    {
        let _bridge = ChildGuard::spawn({
            let mut cmd = ljd_command();
            cmd.arg("--config").arg(&bridge_config).arg("bridge");
            cmd
        })?;
        wait_until(Duration::from_secs(5), || Ok(collector.messages().len() >= 6))?;
    }

    assert_eq!(
        collector.messages(),
        vec![
            "RESUME 001".to_string(),
            "RESUME 002".to_string(),
            "RESUME 003".to_string(),
            "RESUME 004".to_string(),
            "RESUME 005".to_string(),
            "RESUME 006".to_string(),
        ]
    );

    Ok(())
}

#[test]
fn bridge_keep_works_with_file_rotation() -> io::Result<()> {
    let dir = TestDir::new("bridge-file-rotation")?;
    let spool_dir = dir.path().join("spool");
    fs::create_dir_all(&spool_dir)?;
    let ingest_port = free_port()?;
    let replay_port = free_port()?;
    let collector_port = free_port()?;

    let appliance_config = dir.write(
        "appliance-file.conf",
        &format!(
            "output: file\nfile.path: {}\nfile.size: 1\nfile.name: rotation.logjet\ningest.protocol: otlp-http\ningest.listen: 127.0.0.1:{ingest_port}\nreplay.listen: 127.0.0.1:{replay_port}\n",
            spool_dir.display()
        ),
    )?;
    let bridge_config = dir.write(
        "bridge-file.conf",
        &format!("collector.url: http://127.0.0.1:{collector_port}/v1/logs\nupstream.replay: 127.0.0.1:{replay_port}\nupstream.mode: keep\n"),
    )?;

    let _appliance = ChildGuard::spawn({
        let mut cmd = ljd_command();
        cmd.arg("--config").arg(&appliance_config).arg("serve");
        cmd
    })?;
    wait_for_tcp(&format!("127.0.0.1:{ingest_port}"), Duration::from_secs(5))?;
    wait_for_tcp(&format!("127.0.0.1:{replay_port}"), Duration::from_secs(5))?;

    for index in 1..=5 {
        let message = format!("FILE {index:03} {}", noisy_message(index));
        post_otlp_http(&format!("127.0.0.1:{ingest_port}"), "bridge-file", &message)?;
    }

    let collector = MockCollector::start(collector_port)?;
    let _bridge = ChildGuard::spawn({
        let mut cmd = ljd_command();
        cmd.arg("--config").arg(&bridge_config).arg("bridge");
        cmd
    })?;

    wait_until(Duration::from_secs(5), || Ok(collector.messages().len() >= 5))?;
    assert_eq!(
        collector.messages(),
        vec![
            format!("FILE 001 {}", noisy_message(1)),
            format!("FILE 002 {}", noisy_message(2)),
            format!("FILE 003 {}", noisy_message(3)),
            format!("FILE 004 {}", noisy_message(4)),
            format!("FILE 005 {}", noisy_message(5)),
        ]
    );

    let rotated_count = fs::read_dir(&spool_dir)?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("rotation") && entry.file_name().to_string_lossy().ends_with(".logjet"))
        .count();
    assert!(rotated_count >= 2);

    Ok(())
}

#[test]
fn bridge_resets_saved_state_when_upstream_stream_changes() -> io::Result<()> {
    let dir = TestDir::new("bridge-upstream-reset")?;
    let ingest_port = free_port()?;
    let replay_port = free_port()?;
    let collector_port = free_port()?;
    let state_path = dir.path().join("bridge.state");

    let appliance_alpha = dir.write(
        "appliance-alpha.conf",
        &format!(
            "output: buffer\nbuffer.messages: 64\ningest.protocol: otlp-http\ningest.listen: 127.0.0.1:{ingest_port}\nreplay.listen: 127.0.0.1:{replay_port}\n"
        ),
    )?;
    let appliance_bravo = dir.write(
        "appliance-bravo.conf",
        &format!(
            "output: buffer\nbuffer.messages: 64\ningest.protocol: otlp-http\ningest.listen: 127.0.0.1:{ingest_port}\nreplay.listen: 127.0.0.1:{replay_port}\n"
        ),
    )?;
    let bridge_config = dir.write(
        "bridge-reset.conf",
        &format!(
            "collector.url: http://127.0.0.1:{collector_port}/v1/logs\nupstream.replay: 127.0.0.1:{replay_port}\nupstream.mode: keep\nupstream.state-file: {}\n",
            state_path.display()
        ),
    )?;

    let collector = MockCollector::start(collector_port)?;

    let _bridge = ChildGuard::spawn({
        let mut cmd = ljd_command();
        cmd.arg("--config").arg(&bridge_config).arg("bridge");
        cmd
    })?;

    {
        let _appliance = ChildGuard::spawn({
            let mut cmd = ljd_command();
            cmd.arg("--config").arg(&appliance_alpha).arg("serve");
            cmd
        })?;
        wait_for_tcp(&format!("127.0.0.1:{ingest_port}"), Duration::from_secs(5))?;
        wait_for_tcp(&format!("127.0.0.1:{replay_port}"), Duration::from_secs(5))?;
        post_otlp_http(&format!("127.0.0.1:{ingest_port}"), "bridge-reset", "ALPHA 001")?;
        post_otlp_http(&format!("127.0.0.1:{ingest_port}"), "bridge-reset", "ALPHA 002")?;
        wait_until(Duration::from_secs(5), || Ok(collector.messages().len() >= 2))?;
    }

    wait_until(Duration::from_secs(5), || Ok(TcpStream::connect(format!("127.0.0.1:{replay_port}")).is_err()))?;

    {
        let _appliance = ChildGuard::spawn({
            let mut cmd = ljd_command();
            cmd.arg("--config").arg(&appliance_bravo).arg("serve");
            cmd
        })?;
        wait_for_tcp(&format!("127.0.0.1:{ingest_port}"), Duration::from_secs(5))?;
        wait_for_tcp(&format!("127.0.0.1:{replay_port}"), Duration::from_secs(5))?;
        post_otlp_http(&format!("127.0.0.1:{ingest_port}"), "bridge-reset", "BRAVO 001")?;
        post_otlp_http(&format!("127.0.0.1:{ingest_port}"), "bridge-reset", "BRAVO 002")?;
        wait_until(Duration::from_secs(5), || Ok(collector.messages().len() >= 4))?;
    }

    assert_eq!(collector.messages(), vec!["ALPHA 001".to_string(), "ALPHA 002".to_string(), "BRAVO 001".to_string(), "BRAVO 002".to_string(),]);

    Ok(())
}

#[test]
fn bridge_block_mode_handles_slow_collector_without_losing_order() -> io::Result<()> {
    let dir = TestDir::new("bridge-slow-collector")?;
    let ingest_port = free_port()?;
    let replay_port = free_port()?;
    let collector_port = free_port()?;

    let appliance_config = dir.write(
        "appliance.conf",
        &format!(
            "output: buffer\nbuffer.messages: 256\ningest.protocol: otlp-http\ningest.listen: 127.0.0.1:{ingest_port}\nreplay.listen: 127.0.0.1:{replay_port}\n"
        ),
    )?;
    let bridge_config = dir.write(
        "bridge.conf",
        &format!(
            "collector.url: http://127.0.0.1:{collector_port}/v1/logs\nupstream.replay: 127.0.0.1:{replay_port}\nupstream.mode: keep\nbackpressure.enabled: true\nbackpressure.mode: block\nbackpressure.max-buffered-records: 2\n"
        ),
    )?;

    let _appliance = ChildGuard::spawn({
        let mut cmd = ljd_command();
        cmd.arg("--config").arg(&appliance_config).arg("serve");
        cmd
    })?;
    wait_for_tcp(&format!("127.0.0.1:{ingest_port}"), Duration::from_secs(5))?;
    wait_for_tcp(&format!("127.0.0.1:{replay_port}"), Duration::from_secs(5))?;

    let collector = MockCollector::start_with_delay(collector_port, Duration::from_millis(150))?;
    let _bridge = ChildGuard::spawn({
        let mut cmd = ljd_command();
        cmd.arg("--config").arg(&bridge_config).arg("bridge");
        cmd
    })?;

    for index in 1..=6 {
        post_otlp_http(&format!("127.0.0.1:{ingest_port}"), "bridge-slow", &format!("SLOW {index:03}"))?;
    }

    wait_until(Duration::from_secs(10), || Ok(collector.messages().len() >= 6))?;
    assert_eq!(
        collector.messages(),
        vec![
            "SLOW 001".to_string(),
            "SLOW 002".to_string(),
            "SLOW 003".to_string(),
            "SLOW 004".to_string(),
            "SLOW 005".to_string(),
            "SLOW 006".to_string(),
        ]
    );

    Ok(())
}

#[test]
fn replay_recovers_after_middle_of_file_is_removed() -> io::Result<()> {
    let dir = TestDir::new("replay-corruption")?;
    let spool_dir = dir.path().join("spool");
    fs::create_dir_all(&spool_dir)?;
    let ingest_port = free_port()?;
    let collector_port = free_port()?;

    let appliance_config = dir.write(
        "appliance.conf",
        &format!(
            "output: file\nfile.path: {}\nfile.size: 1024\nfile.name: recover.logjet\ningest.protocol: otlp-http\ningest.listen: 127.0.0.1:{ingest_port}\n",
            spool_dir.display()
        ),
    )?;

    {
        let _appliance = ChildGuard::spawn({
            let mut cmd = ljd_command();
            cmd.arg("--config").arg(&appliance_config).arg("serve");
            cmd
        })?;
        wait_for_tcp(&format!("127.0.0.1:{ingest_port}"), Duration::from_secs(5))?;

        for index in 1..=120 {
            let message = format!("RECOVER {index:03} {}", noisy_message(index));
            post_otlp_http(&format!("127.0.0.1:{ingest_port}"), "replay-corruption", &message)?;
        }
    }

    let spool_file = spool_dir.join("recover.logjet");
    let original = fs::read(&spool_file)?;
    let first_cut = original.len() / 3;
    let second_cut = (original.len() * 2) / 3;
    let mut damaged = Vec::with_capacity(original.len() - (second_cut - first_cut));
    damaged.extend_from_slice(&original[..first_cut]);
    damaged.extend_from_slice(&original[second_cut..]);
    fs::write(&spool_file, damaged)?;

    let collector = MockCollector::start(collector_port)?;
    let status = {
        let mut cmd = ljd_command();
        cmd.arg("replay").arg("--path").arg(&spool_dir).arg("--name").arg("recover.logjet").arg("--dest").arg(format!("127.0.0.1:{collector_port}"));
        cmd.status()?
    };
    assert!(status.success());

    wait_until(Duration::from_secs(5), || Ok(!collector.messages().is_empty()))?;
    let messages = collector.messages();
    assert!(messages.len() < 120);
    assert!(messages.iter().any(|message| message.starts_with("RECOVER 090 ")));

    Ok(())
}

#[test]
fn bridge_forwards_large_payloads_end_to_end() -> io::Result<()> {
    let dir = TestDir::new("bridge-large-payload")?;
    let ingest_port = free_port()?;
    let replay_port = free_port()?;
    let collector_port = free_port()?;

    let appliance_config = dir.write(
        "appliance.conf",
        &format!(
            "output: buffer\nbuffer.messages: 8\ningest.protocol: otlp-http\ningest.listen: 127.0.0.1:{ingest_port}\nreplay.listen: 127.0.0.1:{replay_port}\ningest.max-batch-bytes: 524288\n"
        ),
    )?;
    let bridge_config = dir.write(
        "bridge.conf",
        &format!("collector.url: http://127.0.0.1:{collector_port}/v1/logs\nupstream.replay: 127.0.0.1:{replay_port}\nupstream.mode: keep\n"),
    )?;

    let _appliance = ChildGuard::spawn({
        let mut cmd = ljd_command();
        cmd.arg("--config").arg(&appliance_config).arg("serve");
        cmd
    })?;
    wait_for_tcp(&format!("127.0.0.1:{ingest_port}"), Duration::from_secs(5))?;
    wait_for_tcp(&format!("127.0.0.1:{replay_port}"), Duration::from_secs(5))?;

    let collector = MockCollector::start(collector_port)?;
    let _bridge = ChildGuard::spawn({
        let mut cmd = ljd_command();
        cmd.arg("--config").arg(&bridge_config).arg("bridge");
        cmd
    })?;

    let large_message = format!("LARGE 001 {}", noisy_message(10_001));
    post_otlp_http(&format!("127.0.0.1:{ingest_port}"), "bridge-large", &large_message)?;

    wait_until(Duration::from_secs(5), || Ok(!collector.messages().is_empty()))?;
    assert_eq!(collector.messages(), vec![large_message]);

    Ok(())
}

#[test]
fn multiple_replay_clients_receive_backlog_independently() -> io::Result<()> {
    let dir = TestDir::new("bridge-multi-client")?;
    let ingest_port = free_port()?;
    let replay_port = free_port()?;

    let appliance_config = dir.write(
        "appliance.conf",
        &format!(
            "output: buffer\nbuffer.messages: 64\ningest.protocol: otlp-http\ningest.listen: 127.0.0.1:{ingest_port}\nreplay.listen: 127.0.0.1:{replay_port}\n"
        ),
    )?;

    let _appliance = ChildGuard::spawn({
        let mut cmd = ljd_command();
        cmd.arg("--config").arg(&appliance_config).arg("serve");
        cmd
    })?;
    wait_for_tcp(&format!("127.0.0.1:{ingest_port}"), Duration::from_secs(5))?;
    wait_for_tcp(&format!("127.0.0.1:{replay_port}"), Duration::from_secs(5))?;

    for index in 1..=20 {
        post_otlp_http(&format!("127.0.0.1:{ingest_port}"), "bridge-multi", &format!("MULTI {index:03}"))?;
    }

    let replay_addr = format!("127.0.0.1:{replay_port}");
    let replay_addr_clone = replay_addr.clone();
    let first = thread::spawn(move || replay_messages(&replay_addr, 0, 20));
    let second = thread::spawn(move || replay_messages(&replay_addr_clone, 0, 20));

    let first_messages = first.join().map_err(|_| io::Error::other("first replay thread panicked"))??;
    let second_messages = second.join().map_err(|_| io::Error::other("second replay thread panicked"))??;

    let expected = (1..=20).map(|index| format!("MULTI {index:03}")).collect::<Vec<_>>();
    assert_eq!(first_messages, expected);
    assert_eq!(second_messages, expected);

    Ok(())
}

#[test]
fn replay_client_receives_backlog_then_live_records_without_reconnect() -> io::Result<()> {
    let dir = TestDir::new("bridge-live-handoff")?;
    let ingest_port = free_port()?;
    let replay_port = free_port()?;

    let appliance_config = dir.write(
        "appliance.conf",
        &format!(
            "output: file\nfile.path: {}\nfile.size: 64\nfile.name: handoff.logjet\ningest.protocol: otlp-http\ningest.listen: 127.0.0.1:{ingest_port}\nreplay.listen: 127.0.0.1:{replay_port}\n",
            dir.path().join("spool").display()
        ),
    )?;

    let _appliance = ChildGuard::spawn({
        let mut cmd = ljd_command();
        cmd.arg("--config").arg(&appliance_config).arg("serve");
        cmd
    })?;
    wait_for_tcp(&format!("127.0.0.1:{ingest_port}"), Duration::from_secs(5))?;
    wait_for_tcp(&format!("127.0.0.1:{replay_port}"), Duration::from_secs(5))?;

    for message in ["HANDOFF 001", "HANDOFF 002", "HANDOFF 003"] {
        post_otlp_http(&format!("127.0.0.1:{ingest_port}"), "bridge-live", message)?;
    }

    let mut replay = connect_replay_client(&format!("127.0.0.1:{replay_port}"), 0, false)?;
    assert_eq!(read_replay_message(&mut replay)?, Some("HANDOFF 001".to_string()));
    assert_eq!(read_replay_message(&mut replay)?, Some("HANDOFF 002".to_string()));
    assert_eq!(read_replay_message(&mut replay)?, Some("HANDOFF 003".to_string()));

    thread::sleep(Duration::from_millis(200));
    assert_eq!(read_replay_message(&mut replay)?, None);

    post_otlp_http(&format!("127.0.0.1:{ingest_port}"), "bridge-live", "HANDOFF 004")?;

    replay.set_read_timeout(Some(Duration::from_secs(5)))?;
    assert_eq!(read_replay_message(&mut replay)?, Some("HANDOFF 004".to_string()));

    Ok(())
}

fn noisy_message(seed: usize) -> String {
    let mut value = (seed as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(0xD1B5_4A32_D192_ED03);
    let mut text = String::with_capacity(4096);
    for _ in 0..256 {
        value = value.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        text.push_str(&format!("{value:016x}"));
    }
    text
}
