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

## Replay Request Frame

Replay clients send a request first before the server starts streaming records.

Replay request layout:

1. magic: 8 bytes
2. version: `u8`
3. reserved: 7 bytes
4. from sequence: `u64`, little-endian

Meaning:

- the client already has everything up to `from_seq`
- the server should send only records with sequence greater than `from_seq`

## Semantics

- ingest clients send framed records to `logjetd`
- replay clients first send `from_seq`, then receive framed records from `logjetd`
- `logjetd` does not decode the OTLP payload
- sequence ordering is preserved if producers send ordered sequence numbers
- reconnecting bridge clients can resume from the last forwarded sequence without restarting from zero

## Why It Exists

The current protocol is intentionally small and dependency-light so `logjetd`
can remain easy to build on constrained systems.

It is a transport layer for the current daemon implementation, not the final
external interoperability story.
