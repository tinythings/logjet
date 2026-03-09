use logjet::LogjetReader;

use crate::cli::CountArgs;
use crate::error::Result;
use crate::input::InputHandle;

pub fn run(args: CountArgs) -> Result<()> {
    let predicate = args.predicate.build()?;
    let input = InputHandle::open(&args.input)?;
    let mut reader = LogjetReader::new(input.into_buf_reader());

    let mut count = 0u64;
    while let Some(record) = reader.next_record()? {
        if predicate.matches(&record) {
            count = count.checked_add(1).ok_or(logjet::Error::NumericOverflow("count"))?;
        }
    }

    println!("{count}");
    Ok(())
}
