use std::io::{self, BufReader, BufWriter, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::config::Config;
use crate::protocol::read_record;
use crate::spool::Spool;

#[derive(Debug, Clone)]
pub struct DaemonConfig {
    pub config: Config,
    pub config_path: PathBuf,
}

pub fn serve(config: DaemonConfig) -> io::Result<()> {
    let spool = Arc::new(Mutex::new(Spool::open(config.config.storage.clone())?));

    let replay_spool = Arc::clone(&spool);
    let replay_addr = config.config.replay_addr.clone();
    let poll_interval_ms = config.config.poll_interval_ms;

    let replay_thread = thread::Builder::new()
        .name("logjetd-replay".to_string())
        .spawn(move || replay_loop(replay_addr, replay_spool, poll_interval_ms))?;

    eprintln!("logjetd using config {}", config.config_path.display());
    ingest_loop(config.config.ingest_addr, spool)?;
    replay_thread
        .join()
        .map_err(|_| io::Error::other("replay listener thread panicked"))?
}

fn ingest_loop(bind_addr: String, spool: Arc<Mutex<Spool>>) -> io::Result<()> {
    let listener = TcpListener::bind(&bind_addr)?;
    eprintln!("logjetd ingest listening on {bind_addr}");

    for stream in listener.incoming() {
        let stream = stream?;
        let spool = Arc::clone(&spool);
        thread::Builder::new()
            .name("logjetd-ingest-client".to_string())
            .spawn(move || {
                if let Err(err) = handle_ingest_client(stream, spool) {
                    eprintln!("logjetd ingest client error: {err}");
                }
            })?;
    }

    Ok(())
}

fn replay_loop(bind_addr: String, spool: Arc<Mutex<Spool>>, poll_interval_ms: u64) -> io::Result<()> {
    let listener = TcpListener::bind(&bind_addr)?;
    eprintln!("logjetd replay listening on {bind_addr}");

    for stream in listener.incoming() {
        let stream = stream?;
        let spool = Arc::clone(&spool);
        thread::Builder::new()
            .name("logjetd-replay-client".to_string())
            .spawn(move || {
                if let Err(err) = handle_replay_client(stream, spool, poll_interval_ms) {
                    eprintln!("logjetd replay client error: {err}");
                }
            })?;
    }

    Ok(())
}

fn handle_ingest_client(stream: TcpStream, spool: Arc<Mutex<Spool>>) -> io::Result<()> {
    let peer = stream.peer_addr().ok();
    let mut reader = BufReader::new(stream);

    while let Some(record) = read_record(&mut reader)? {
        let mut spool = spool
            .lock()
            .map_err(|_| io::Error::other("spool mutex poisoned"))?;
        spool.append(record)?;
    }

    if let Some(peer) = peer {
        eprintln!("logjetd ingest disconnected: {peer}");
    }
    Ok(())
}

fn handle_replay_client(
    stream: TcpStream,
    spool: Arc<Mutex<Spool>>,
    poll_interval_ms: u64,
) -> io::Result<()> {
    let mut writer = BufWriter::new(stream);
    let mut last_seq = 0u64;
    let sleep = Duration::from_millis(poll_interval_ms);

    loop {
        let sent_any = {
            let spool = spool
                .lock()
                .map_err(|_| io::Error::other("spool mutex poisoned"))?;
            spool.replay_since(&mut writer, &mut last_seq)?
        };

        if sent_any {
            writer.flush()?;
        } else {
            writer.flush()?;
            thread::sleep(sleep);
        }
    }
}
