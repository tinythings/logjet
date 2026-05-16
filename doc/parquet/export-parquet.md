# `ljx --export parquet`

`ljx --export parquet` converts one `.logjet` file into a Parquet file through the external exporter plugin.

## Plugin Discovery

The Parquet exporter is a shared-library plugin (`libljx_parquet_exporter.so`) that implements the stable `liblogjet::export` C ABI. `ljx` discovers it via:

1. `LJX_EXPORTER_PATH` environment variable (explicit paths or directories)
2. `./exporters` relative to working directory
3. Paths relative to the `ljx` executable
4. On Unix: `/usr/lib/logjet/exporters` and `/usr/lib/logjet`

## Usage

```bash
# Export one .logjet file to Parquet
ljx --export parquet input.logjet -o output.parquet --force

# With plugin explicitly specified
LJX_EXPORTER_PATH=/usr/lib/logjet/exporters/libljx_parquet_exporter.so ljx --export parquet input.logjet -o output.parquet
```

## Unified Schema

The Parquet exporter uses a single fixed schema that covers all three OTLP signal types. When a signal does not use a column, it is simply null. This guarantees a predictable output schema regardless of input content.

| Column | Type | Signal | Description |
|---|---|---|---|
| `sequence` | `UInt64` (required) | All | Internal logjet sequence number |
| `timestamp_unix_ns` | `UInt64` (nullable) | All | Record timestamp in nanoseconds since epoch |
| `signal_type` | `Utf8` (required) | All | `"logs"`, `"metrics"`, or `"traces"` |
| `observed_timestamp_unix_ns` | `UInt64` (nullable) | Logs | Log record observed timestamp |
| `trace_id` | `Utf8` (nullable) | Logs, Traces | Hex-encoded trace ID |
| `span_id` | `Utf8` (nullable) | Logs, Traces | Hex-encoded span ID |
| `trace_flags` | `UInt32` (nullable) | Logs, Traces | Trace flags |
| `severity_number` | `Int32` (nullable) | Logs | OTLP severity number |
| `severity_text` | `Utf8` (nullable) | Logs | Severity text (e.g. `"INFO"`) |
| `body_kind` | `Utf8` (nullable) | Logs | `string`, `int`, `bool`, `double`, `bytes`, `array`, `kvlist`, `empty` |
| `body_string` | `Utf8` (nullable) | Logs | Body when it is a plain string |
| `body_json` | `Utf8` (nullable) | Logs | JSON representation of non-string bodies |
| `metric_name` | `Utf8` (nullable) | Metrics | Metric instrument name |
| `metric_description` | `Utf8` (nullable) | Metrics | Metric description |
| `metric_unit` | `Utf8` (nullable) | Metrics | Metric unit |
| `metric_type` | `Utf8` (nullable) | Metrics | `Gauge`, `Sum`, `Histogram`, `ExponentialHistogram`, `Summary`, `Unknown` |
| `metric_value_number` | `Float64` (nullable) | Metrics | Value for Gauge/Sum datapoints |
| `metric_value_count` | `UInt64` (nullable) | Metrics | Count for Histogram/Summary datapoints |
| `metric_value_sum` | `Float64` (nullable) | Metrics | Sum for Histogram/Summary datapoints |
| `is_monotonic` | `Boolean` (nullable) | Metrics | True for monotonic Sum metrics |
| `aggregation_temporality` | `Int32` (nullable) | Metrics | OTLP aggregation temporality code |
| `span_name` | `Utf8` (nullable) | Traces | Span name |
| `span_kind` | `Utf8` (nullable) | Traces | `Internal`, `Server`, `Client`, `Producer`, `Consumer` |
| `parent_span_id` | `Utf8` (nullable) | Traces | Hex-encoded parent span ID |
| `start_time_unix_ns` | `UInt64` (nullable) | Traces, Metrics | Start time in nanoseconds |
| `end_time_unix_ns` | `UInt64` (nullable) | Traces | End time in nanoseconds |
| `duration_ns` | `UInt64` (nullable) | Traces | Computed duration (`end - start`) |
| `status_code` | `Int32` (nullable) | Traces | OTLP span status code |
| `status_message` | `Utf8` (nullable) | Traces | Span status message |
| `service_name` | `Utf8` (nullable) | All | Extracted from `service.name` resource attribute |
| `scope_name` | `Utf8` (nullable) | All | Instrumentation scope name |
| `scope_version` | `Utf8` (nullable) | All | Instrumentation scope version |
| `resource_attributes_json` | `Utf8` (nullable) | All | JSON object of resource attributes |
| `scope_attributes_json` | `Utf8` (nullable) | All | JSON object of scope attributes |
| `log_attributes_json` | `Utf8` (nullable) | Logs | JSON object of log record attributes |
| `span_attributes_json` | `Utf8` (nullable) | Traces | JSON object of span attributes |
| `event_name` | `Utf8` (nullable) | Logs | Log record event name |

## Design Decisions

- **One row per datapoint / span / log record**: A single `.logjet` batch may produce multiple Parquet rows. This is the same granularity as NDJSON export.
- **Fixed schema**: All columns are always present. Null-only columns cost negligible space in Parquet due to run-length encoding of definition levels, and a fixed schema is far more convenient for downstream analytics tools.
- **No aggregation or enrichment**: Values are extracted as-is from the stored OTLP protobuf. No unit conversion, no histogram bucketing, no semantic interpretation.

## Writer Options

- `output.row-group-rows`: target rows per Parquet row group (default 8192)
- `output.compression`: `zstd` (default) or `uncompressed`

## Limits

- Histogram bucket boundaries and counts are not extracted into Parquet rows. Only `count` and `sum` are preserved. Use raw OTLP replay if you need full histogram detail.
- Metrics exemplars are not extracted.

## Build

```bash
cargo build -p ljx-parquet-exporter
```

The plugin is a `cdylib` and must be discoverable by `ljx` at runtime.
