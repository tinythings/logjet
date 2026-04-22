use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use logjet::{LogjetWriter, RecordType, WriterConfig};
use otlp_demo::build_message_request_for_service;
use prost::Message;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let output = match args.next() {
        Some(path) => PathBuf::from(path),
        None => {
            eprintln!("usage: visual-logtail-emitter <output.logjet> [seed]");
            std::process::exit(2);
        }
    };
    let seed = args.next().map(|value| value.parse::<u64>()).transpose()?.unwrap_or(0x51a1_7a11);
    let mut rng = Lcg::new(seed);
    let mut seq = 1u64;

    loop {
        let service = SERVICES[rng.next_index(SERVICES.len())];
        let severity = LEVELS[rng.next_index(LEVELS.len())];
        let message = format_message(seq, &mut rng);
        let request = build_message_request_for_service(seq, service, severity, message.clone());

        let file = OpenOptions::new().create(true).append(true).open(&output)?;
        let writer = BufWriter::new(file);
        let mut logjet = LogjetWriter::with_config(writer, WriterConfig::default());
        logjet.push(RecordType::Logs, seq, unix_time_nanos(seq), &request.encode_to_vec())?;
        let mut writer = logjet.into_inner()?;
        writer.flush()?;

        eprintln!("#{seq} {severity:>5} {service}: {message}");
        seq = seq.saturating_add(1);
        thread::sleep(Duration::from_millis(500));
    }
}

const SERVICES: &[&str] = &["visual-tail", "garage-rig", "bridge-alpha", "coffee-daemon", "night-shift", "kill-bill"];
const LEVELS: &[&str] = &["trace", "debug", "info", "warn", "error"];
const SUBJECTS: &[&str] = &[
    "reindexed the replay cursor",
    "shook loose a sleepy bridge",
    "tickled the spool rotation",
    "poked the checksum goblin",
    "nudged the ingest guardrail",
    "confused the on-call dashboard",
    "reheated a stale batch",
    "misplaced a highly motivated packet",
];
const CONTEXTS: &[&str] = &[
    "during a suspiciously calm deploy",
    "while the collector blinked twice",
    "after a ceremonial config reload",
    "under polite backpressure",
    "while the spool muttered darkly",
    "after a coffee-powered rollback",
    "before the bridge could complain",
    "while the checksum looked offended",
];
const OUTCOMES: &[&str] = &[
    "and the logs kept flowing",
    "but the operator remained unconvinced",
    "so tail mode had something juicy to chew",
    "and the file grew another tiny block",
    "before the daemon could get grumpy",
    "yet the replay queue stayed weirdly serene",
    "and everyone blamed cosmic rays anyway",
    "with absolutely no paperwork filed",
];

fn format_message(seq: u64, rng: &mut Lcg) -> String {
    let subject = SUBJECTS[rng.next_index(SUBJECTS.len())];
    let context = CONTEXTS[rng.next_index(CONTEXTS.len())];
    let outcome = OUTCOMES[rng.next_index(OUTCOMES.len())];
    format!("#{seq}: {subject} {context} {outcome}")
}

fn unix_time_nanos(seq: u64) -> u64 {
    let base = 1_777_000_000_000_000_000u64;
    base.saturating_add(seq.saturating_mul(500_000_000))
}

struct Lcg {
    state: u64,
}

impl Lcg {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_mul(6364136223846793005).wrapping_add(1);
        self.state
    }

    fn next_index(&mut self, len: usize) -> usize {
        (self.next() % len as u64) as usize
    }
}
