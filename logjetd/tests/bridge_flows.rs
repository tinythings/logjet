mod common;

use std::fs;
use std::io;
use std::net::TcpStream;
use std::thread;
use std::time::Duration;

use common::{
    ChildGuard, MockCollector, TestDir, free_port, logjetd_command, post_otlp_http, replay_messages,
    wait_for_tcp, wait_until,
};

#[test]
fn bridge_keep_forwards_backlog_in_order() -> io::Result<()> {
    let dir = TestDir::new("bridge-keep")?;
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
        &format!(
            "collector.url: http://127.0.0.1:{collector_port}/v1/logs\nupstream.replay: 127.0.0.1:{replay_port}\nupstream.mode: keep\n"
        ),
    )?;

    let _appliance = ChildGuard::spawn({
        let mut cmd = logjetd_command();
        cmd.arg("--config").arg(&appliance_config).arg("serve");
        cmd
    })?;
    wait_for_tcp(&format!("127.0.0.1:{ingest_port}"), Duration::from_secs(5))?;
    wait_for_tcp(&format!("127.0.0.1:{replay_port}"), Duration::from_secs(5))?;

    for message in ["KEEP 001", "KEEP 002", "KEEP 003"] {
        post_otlp_http(&format!("127.0.0.1:{ingest_port}"), "bridge-keep", message)?;
    }

    let collector = MockCollector::start(collector_port)?;
    let _bridge = ChildGuard::spawn({
        let mut cmd = logjetd_command();
        cmd.arg("--config").arg(&bridge_config).arg("bridge");
        cmd
    })?;

    wait_until(Duration::from_secs(5), || Ok(collector.messages().len() >= 3))?;
    assert_eq!(
        collector.messages(),
        vec![
            "KEEP 001".to_string(),
            "KEEP 002".to_string(),
            "KEEP 003".to_string()
        ]
    );

    let retained = replay_messages(&format!("127.0.0.1:{replay_port}"), 0, 3)?;
    assert_eq!(
        retained,
        vec![
            "KEEP 001".to_string(),
            "KEEP 002".to_string(),
            "KEEP 003".to_string()
        ]
    );

    Ok(())
}

#[test]
fn bridge_drain_consumes_upstream_records() -> io::Result<()> {
    let dir = TestDir::new("bridge-drain")?;
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
        &format!(
            "collector.url: http://127.0.0.1:{collector_port}/v1/logs\nupstream.replay: 127.0.0.1:{replay_port}\nupstream.mode: drain\n"
        ),
    )?;

    let _appliance = ChildGuard::spawn({
        let mut cmd = logjetd_command();
        cmd.arg("--config").arg(&appliance_config).arg("serve");
        cmd
    })?;
    wait_for_tcp(&format!("127.0.0.1:{ingest_port}"), Duration::from_secs(5))?;
    wait_for_tcp(&format!("127.0.0.1:{replay_port}"), Duration::from_secs(5))?;

    for message in ["DRAIN 001", "DRAIN 002", "DRAIN 003"] {
        post_otlp_http(&format!("127.0.0.1:{ingest_port}"), "bridge-drain", message)?;
    }

    let collector = MockCollector::start(collector_port)?;
    let _bridge = ChildGuard::spawn({
        let mut cmd = logjetd_command();
        cmd.arg("--config").arg(&bridge_config).arg("bridge");
        cmd
    })?;

    wait_until(Duration::from_secs(5), || Ok(collector.messages().len() >= 3))?;
    assert_eq!(
        collector.messages(),
        vec![
            "DRAIN 001".to_string(),
            "DRAIN 002".to_string(),
            "DRAIN 003".to_string()
        ]
    );

    wait_until(Duration::from_secs(5), || {
        Ok(replay_messages(&format!("127.0.0.1:{replay_port}"), 0, 1)?.is_empty())
    })?;

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
        let mut cmd = logjetd_command();
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
            let mut cmd = logjetd_command();
            cmd.arg("--config").arg(&bridge_config).arg("bridge");
            cmd
        })?;
        wait_until(Duration::from_secs(5), || Ok(collector.messages().len() >= 3))?;
    }

    for message in ["RESUME 004", "RESUME 005", "RESUME 006"] {
        post_otlp_http(&format!("127.0.0.1:{ingest_port}"), "bridge-resume", message)?;
    }

    {
        let _bridge = ChildGuard::spawn({
            let mut cmd = logjetd_command();
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
        &format!(
            "collector.url: http://127.0.0.1:{collector_port}/v1/logs\nupstream.replay: 127.0.0.1:{replay_port}\nupstream.mode: keep\n"
        ),
    )?;

    let _appliance = ChildGuard::spawn({
        let mut cmd = logjetd_command();
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
        let mut cmd = logjetd_command();
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
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("rotation")
                && entry.file_name().to_string_lossy().ends_with(".logjet")
        })
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
        let mut cmd = logjetd_command();
        cmd.arg("--config").arg(&bridge_config).arg("bridge");
        cmd
    })?;

    {
        let _appliance = ChildGuard::spawn({
            let mut cmd = logjetd_command();
            cmd.arg("--config").arg(&appliance_alpha).arg("serve");
            cmd
        })?;
        wait_for_tcp(&format!("127.0.0.1:{ingest_port}"), Duration::from_secs(5))?;
        wait_for_tcp(&format!("127.0.0.1:{replay_port}"), Duration::from_secs(5))?;
        post_otlp_http(&format!("127.0.0.1:{ingest_port}"), "bridge-reset", "ALPHA 001")?;
        post_otlp_http(&format!("127.0.0.1:{ingest_port}"), "bridge-reset", "ALPHA 002")?;
        wait_until(Duration::from_secs(5), || Ok(collector.messages().len() >= 2))?;
    }

    wait_until(Duration::from_secs(5), || {
        Ok(TcpStream::connect(format!("127.0.0.1:{replay_port}")).is_err())
    })?;

    {
        let _appliance = ChildGuard::spawn({
            let mut cmd = logjetd_command();
            cmd.arg("--config").arg(&appliance_bravo).arg("serve");
            cmd
        })?;
        wait_for_tcp(&format!("127.0.0.1:{ingest_port}"), Duration::from_secs(5))?;
        wait_for_tcp(&format!("127.0.0.1:{replay_port}"), Duration::from_secs(5))?;
        post_otlp_http(&format!("127.0.0.1:{ingest_port}"), "bridge-reset", "BRAVO 001")?;
        post_otlp_http(&format!("127.0.0.1:{ingest_port}"), "bridge-reset", "BRAVO 002")?;
        wait_until(Duration::from_secs(5), || Ok(collector.messages().len() >= 4))?;
    }

    assert_eq!(
        collector.messages(),
        vec![
            "ALPHA 001".to_string(),
            "ALPHA 002".to_string(),
            "BRAVO 001".to_string(),
            "BRAVO 002".to_string(),
        ]
    );

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
        let mut cmd = logjetd_command();
        cmd.arg("--config").arg(&appliance_config).arg("serve");
        cmd
    })?;
    wait_for_tcp(&format!("127.0.0.1:{ingest_port}"), Duration::from_secs(5))?;
    wait_for_tcp(&format!("127.0.0.1:{replay_port}"), Duration::from_secs(5))?;

    let collector = MockCollector::start_with_delay(collector_port, Duration::from_millis(150))?;
    let _bridge = ChildGuard::spawn({
        let mut cmd = logjetd_command();
        cmd.arg("--config").arg(&bridge_config).arg("bridge");
        cmd
    })?;

    for index in 1..=6 {
        post_otlp_http(
            &format!("127.0.0.1:{ingest_port}"),
            "bridge-slow",
            &format!("SLOW {index:03}"),
        )?;
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
            let mut cmd = logjetd_command();
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
        let mut cmd = logjetd_command();
        cmd.arg("replay")
            .arg("--path")
            .arg(&spool_dir)
            .arg("--name")
            .arg("recover.logjet")
            .arg("--dest")
            .arg(format!("127.0.0.1:{collector_port}"));
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
        &format!(
            "collector.url: http://127.0.0.1:{collector_port}/v1/logs\nupstream.replay: 127.0.0.1:{replay_port}\nupstream.mode: keep\n"
        ),
    )?;

    let _appliance = ChildGuard::spawn({
        let mut cmd = logjetd_command();
        cmd.arg("--config").arg(&appliance_config).arg("serve");
        cmd
    })?;
    wait_for_tcp(&format!("127.0.0.1:{ingest_port}"), Duration::from_secs(5))?;
    wait_for_tcp(&format!("127.0.0.1:{replay_port}"), Duration::from_secs(5))?;

    let collector = MockCollector::start(collector_port)?;
    let _bridge = ChildGuard::spawn({
        let mut cmd = logjetd_command();
        cmd.arg("--config").arg(&bridge_config).arg("bridge");
        cmd
    })?;

    let large_message = format!("LARGE 001 {}", noisy_message(10_001));
    post_otlp_http(
        &format!("127.0.0.1:{ingest_port}"),
        "bridge-large",
        &large_message,
    )?;

    wait_until(Duration::from_secs(5), || Ok(collector.messages().len() >= 1))?;
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
        let mut cmd = logjetd_command();
        cmd.arg("--config").arg(&appliance_config).arg("serve");
        cmd
    })?;
    wait_for_tcp(&format!("127.0.0.1:{ingest_port}"), Duration::from_secs(5))?;
    wait_for_tcp(&format!("127.0.0.1:{replay_port}"), Duration::from_secs(5))?;

    for index in 1..=20 {
        post_otlp_http(
            &format!("127.0.0.1:{ingest_port}"),
            "bridge-multi",
            &format!("MULTI {index:03}"),
        )?;
    }

    let replay_addr = format!("127.0.0.1:{replay_port}");
    let replay_addr_clone = replay_addr.clone();
    let first = thread::spawn(move || replay_messages(&replay_addr, 0, 20));
    let second = thread::spawn(move || replay_messages(&replay_addr_clone, 0, 20));

    let first_messages = first
        .join()
        .map_err(|_| io::Error::other("first replay thread panicked"))??;
    let second_messages = second
        .join()
        .map_err(|_| io::Error::other("second replay thread panicked"))??;

    let expected = (1..=20)
        .map(|index| format!("MULTI {index:03}"))
        .collect::<Vec<_>>();
    assert_eq!(first_messages, expected);
    assert_eq!(second_messages, expected);

    Ok(())
}

fn noisy_message(seed: usize) -> String {
    let mut value = (seed as u64)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(0xD1B5_4A32_D192_ED03);
    let mut text = String::with_capacity(4096);
    for _ in 0..256 {
        value = value
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        text.push_str(&format!("{value:016x}"));
    }
    text
}
