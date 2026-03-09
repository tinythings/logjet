use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use logjet::{LogjetWriter, RecordType, WriterConfig};
use otlp_demo::build_message_request_for_service;
use prost::Message;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let output = match args.next() {
        Some(path) => PathBuf::from(path),
        None => {
            eprintln!("usage: otlp-random-logjet-generator <output.logjet> [count] [seed]");
            std::process::exit(2);
        }
    };
    let count = args
        .next()
        .map(|value| value.parse::<u64>())
        .transpose()?
        .unwrap_or(1000);
    let seed = args
        .next()
        .map(|value| value.parse::<u64>())
        .transpose()?
        .unwrap_or(0x5eed_1234_u64);

    let file = File::create(&output)?;
    let writer = BufWriter::new(file);
    let mut logjet = LogjetWriter::with_config(writer, WriterConfig::default());
    let mut rng = Lcg::new(seed);

    for seq in 1..=count {
        let service = SERVICES[rng.next_index(SERVICES.len())];
        let severity = LEVELS[rng.next_index(LEVELS.len())];
        let message = format_message(seq, &mut rng);
        let request = build_message_request_for_service(seq, service, severity, message);
        logjet.push(RecordType::Logs, seq, unix_time_nanos(seq), &request.encode_to_vec())?;
    }

    let mut writer = logjet.into_inner()?;
    writer.flush()?;
    println!("wrote {count} random log records to {}", output.display());
    Ok(())
}

const SERVICES: &[&str] = &[
    "bofh-emitter",
    "kill-bill",
    "garage-rig",
    "bridge-alpha",
    "night-shift",
    "coffee-daemon",
];

const LEVELS: &[&str] = &["trace", "debug", "info", "warn", "error"];

const SUBJECTS: &[&str] = &[
    "rebooted node",
    "dropped packet train",
    "rewired pipeline",
    "latched failover switch",
    "purged stale checkpoint",
    "stalled replay queue",
    "overfed metrics sink",
    "misread tape robot",
];

const CONTEXTS: &[&str] = &[
    "after a coffee spill",
    "during a midnight deploy",
    "under synthetic backpressure",
    "while testing replay recovery",
    "after rotating the file segment",
    "while the collector blinked twice",
    "after a heroic config change",
    "while tracing vanished quietly",
];

const OUTCOMES: &[&str] = &[
    "and everything looked suspiciously fine",
    "and the bridge resumed from the right offset",
    "but the checksum still looked offended",
    "so the operator blamed cosmic rays",
    "and the daemon recovered without drama",
    "yet the dashboard remained emotionally unavailable",
    "and the logs kept flowing anyway",
    "before the intern touched production again",
];

fn format_message(seq: u64, rng: &mut Lcg) -> String {
    let subject = SUBJECTS[rng.next_index(SUBJECTS.len())];
    let context = CONTEXTS[rng.next_index(CONTEXTS.len())];
    let outcome = OUTCOMES[rng.next_index(OUTCOMES.len())];
    format!("#{seq}: {subject} {context} {outcome}")
}

fn unix_time_nanos(seq: u64) -> u64 {
    let base = 1_773_000_000_000_000_000u64;
    base.saturating_add(seq.saturating_mul(1_000_000))
}

struct Lcg {
    state: u64,
}

impl Lcg {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1);
        self.state
    }

    fn next_index(&mut self, len: usize) -> usize {
        (self.next() % len as u64) as usize
    }
}
