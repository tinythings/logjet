# Current Wire Protocol

`logjetd` currently uses a small custom TCP wire protocol between ingest clients
and replay clients.

This is not OTLP/gRPC and not OTLP/HTTP.

## Record Frame

Each record on the wire is:

1. magic: 8 bytes
2. version: `u8`
3. record type: `u8`
4. reserved: `u16`
5. sequence: `u64`, little-endian
6. timestamp ns: `u64`, little-endian
7. payload length: `u32`, little-endian
8. payload bytes

Payload bytes are raw OTLP protobuf bytes.

## Semantics

- ingest clients send framed records to `logjetd`
- replay clients receive the same framed records from `logjetd`
- `logjetd` does not decode the OTLP payload
- sequence ordering is preserved if producers send ordered sequence numbers

## Why It Exists

The current protocol is intentionally small and dependency-light so `logjetd`
can remain easy to build on constrained systems.

It is a transport layer for the current daemon implementation, not the final
external interoperability story.
