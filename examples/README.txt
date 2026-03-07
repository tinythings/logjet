Examples in this folder show the basic write, read, and recovery flow for the
container.

Files

- write_file.rs
  Creates `telemetry.logjet` in the current working directory and writes a few
  fake OTLP protobuf payloads into it.

- read_file.rs
  Opens `telemetry.logjet`, replays records sequentially, and prints record
  metadata.

- recover_file.rs
  Opens `telemetry.logjet`, replays whatever valid blocks remain, and prints
  recovery statistics. This is useful after manually corrupting bytes in the
  file to observe forward resynchronization.

How to run

1. Write a sample file:
   cargo run --example write_file

2. Read it back:
   cargo run --example read_file

3. Test recovery behavior:
   First corrupt some bytes in `telemetry.logjet`, then run:
   cargo run --example recover_file

Notes

- Run commands from the crate root.
- The examples use fake payload bytes. In production, pass raw OTLP protobuf
  batch bytes as the record payload.
- The output file path is relative to the current working directory.
