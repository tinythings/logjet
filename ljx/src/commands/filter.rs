use std::io::Write;

use logjet::{LogjetReader, LogjetWriter, WriterConfig};

use crate::cli::FilterArgs;
use crate::error::Result;
use crate::input::{InputHandle, open_output};

pub fn run(args: FilterArgs) -> Result<()> {
    let predicate = args.predicate.build()?;
    let input = InputHandle::open(&args.input)?;
    let output = open_output(&args.output)?;
    let config = WriterConfig {
        block_target_size: args.block_target_size,
        codec: args.codec.into(),
        sync_marker: logjet::DEFAULT_SYNC_MARKER,
    };

    let mut reader = LogjetReader::new(input.into_buf_reader());
    let mut writer = LogjetWriter::with_config(output, config);

    while let Some(record) = reader.next_record()? {
        if predicate.matches(&record) {
            writer.push(
                record.record_type,
                record.seq,
                record.ts_unix_ns,
                &record.payload,
            )?;
        }
    }

    let mut output = writer.into_inner()?;
    output.flush()?;
    Ok(())
}
