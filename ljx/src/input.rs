use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{Error, Result};

pub struct InputHandle {
    file: File,
    temp_path: Option<PathBuf>,
}

impl InputHandle {
    pub fn open(path: &Path) -> Result<Self> {
        if path == Path::new("-") { Self::from_stdin() } else { Ok(Self { file: File::open(path)?, temp_path: None }) }
    }

    pub fn into_buf_reader(self) -> BufReader<Self> {
        BufReader::new(self)
    }

    fn from_stdin() -> Result<Self> {
        let path = create_temp_path()?;
        let file = OpenOptions::new().read(true).write(true).create_new(true).open(&path)?;

        let mut writer = BufWriter::new(file);
        let mut stdin = io::stdin().lock();
        io::copy(&mut stdin, &mut writer)?;

        let mut file = writer.into_inner().map_err(io::Error::other)?;
        file.seek(SeekFrom::Start(0))?;

        Ok(Self { file, temp_path: Some(path) })
    }
}

impl Read for InputHandle {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.file.read(buf)
    }
}

impl Seek for InputHandle {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.file.seek(pos)
    }
}

impl Drop for InputHandle {
    fn drop(&mut self) {
        if let Some(path) = &self.temp_path {
            let _ = std::fs::remove_file(path);
        }
    }
}

pub fn open_output(path: &Path) -> Result<Box<dyn Write>> {
    if path == Path::new("-") { Ok(Box::new(BufWriter::new(io::stdout().lock()))) } else { Ok(Box::new(BufWriter::new(File::create(path)?))) }
}

fn create_temp_path() -> Result<PathBuf> {
    let mut attempt = 0u32;
    let base = std::env::temp_dir();
    let pid = std::process::id();
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|err| Error::Usage(format!("system clock error: {err}")))?.as_nanos();

    loop {
        let candidate = base.join(format!("ljx-stdin-{pid}-{nanos}-{attempt}.logjet"));
        match OpenOptions::new().write(true).create_new(true).open(&candidate) {
            Ok(file) => {
                drop(file);
                return Ok(candidate);
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                attempt = attempt.checked_add(1).ok_or(Error::Usage("temporary file naming overflow".to_string()))?;
            }
            Err(err) => return Err(err.into()),
        }
    }
}
