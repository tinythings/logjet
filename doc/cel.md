# CEL Querying

`ljx` supports [CEL (Common Expression Language)][cel-spec] for querying
OpenTelemetry attributes inside `.logjet` files. CEL queries are evaluated
against decoded OTLP records — resource attributes, scope attributes,
record-level attributes, and signal-specific fields.

The CEL engine is provided by the `ljcel` crate, embedded directly in `ljx`.

[cel-spec]: https://github.com/google/cel-spec

## CLI Usage

Use `--cel` on any `ljx` command that accepts filters. Repeat for AND semantics
(all expressions must match):

```text
ljx telemetry.logjet --cel 'severity_number >= 17'
ljx count telemetry.logjet --cel 'service_name == "my-service"'
ljx filter telemetry.logjet -o errors.logjet --cel 'severity_number >= 13 && body.contains("timeout")'
ljx telemetry.logjet --cel 'severity_number >= 13' --cel 'body.contains("error")'
```

`--cel` works alongside `--grep`, `--fixed-string`, and field filters:

```text
ljx telemetry.logjet --cel 'severity_number >= 13' --service my-service -F timeout
```

## TUI Usage

In `ljx view`, the filter mode cycles between **CEL**, **strings**, and **regex**.
Press Up or Down while the search bar is focused. The default mode is CEL.

```
CEL → strings → regex  (Up / Down cycles through all three)
```

The search bar label reflects the active mode:

```text
Filter (CEL):
Filter (strings):
Filter (regex):
```

Typing a filter expression and pressing Enter applies it immediately. Bare
text without leading `-` is dispatched to the active filter mode:

| Mode | Input | Treated as |
|------|-------|-----------|
| CEL | `severity_number >= 17` | CEL expression |
| strings | `timeout` | literal substring |
| regex | `timeout\|deadline` | regex |

## Variable Reference

CEL variables are exposed for every log record, metric data point, or span.
Type names follow CEL conventions: `int`, `double`, `string`, `bool`,
`map(string, dyn)`, `list(dyn)`.

### Log Records

| Variable | Type | Source |
|----------|------|--------|
| `body` | string | `LogRecord.body.string_value` |
| `severity_text` | string | `LogRecord.severity_text` |
| `severity_number` | int | `LogRecord.severity_number` |
| `service_name` | string | `Resource.attributes["service.name"]` |
| `scope_name` | string | `InstrumentationScope.name` |
| `event_name` | string | `LogRecord.event_name` |
| `time_unix_nano` | int | `LogRecord.time_unix_nano` |
| `observed_time_unix_nano` | int | `LogRecord.observed_time_unix_nano` |
| `trace_id` | string | `LogRecord.trace_id` (hex) |
| `span_id` | string | `LogRecord.span_id` (hex) |
| `flags` | int | `LogRecord.flags` |
| `resource` | map | `Resource.attributes` → `resource["key"]` |
| `scope` | map | `InstrumentationScope.attributes` → `scope["key"]` |
| `attributes` | map | `LogRecord.attributes` → `attributes["key"]` |

Map keys preserve the original attribute key including dots.
Use bracket notation: `resource["service.name"]`, `attributes["http.status_code"]`.

### Metric Data Points

| Variable | Type | Source |
|----------|------|--------|
| `metric_name` | string | `Metric.name` |
| `metric_unit` | string | `Metric.unit` |
| `metric_description` | string | `Metric.description` |
| `metric_type` | string | `"Gauge"`, `"Sum"`, `"Histogram"`, `"ExponentialHistogram"`, or `"Summary"` |
| `value` | double | `NumberDataPoint.value` |
| `count` | int | `HistogramDataPoint.count` / `SummaryDataPoint.count` (0 for Gauge/Sum) |
| `sum` | double | `HistogramDataPoint.sum` / `SummaryDataPoint.sum` (0 for Gauge/Sum) |
| `time_unix_nano` | int | `NumberDataPoint.time_unix_nano` |
| `service_name` | string | `Resource.attributes["service.name"]` |
| `scope_name` | string | `InstrumentationScope.name` |
| `resource` | map | `Resource.attributes` → `resource["key"]` |
| `scope` | map | `InstrumentationScope.attributes` → `scope["key"]` |
| `attributes` | map | `NumberDataPoint.attributes` → `attributes["key"]` |

### Span Records

| Variable | Type | Source |
|----------|------|--------|
| `name` | string | `Span.name` |
| `kind` | int | `Span.kind` (1=Internal, 2=Server, 3=Client, 4=Producer, 5=Consumer) |
| `trace_id` | string | `Span.trace_id` (hex) |
| `span_id` | string | `Span.span_id` (hex) |
| `parent_span_id` | string | `Span.parent_span_id` (hex, empty for root spans) |
| `start_time_unix_nano` | int | `Span.start_time_unix_nano` |
| `end_time_unix_nano` | int | `Span.end_time_unix_nano` |
| `duration_ns` | int | `end_time_unix_nano - start_time_unix_nano` |
| `status_code` | int | `Status.code` (0=Unset, 1=Ok, 2=Error) |
| `status_message` | string | `Status.message` |
| `service_name` | string | `Resource.attributes["service.name"]` |
| `scope_name` | string | `InstrumentationScope.name` |
| `resource` | map | `Resource.attributes` → `resource["key"]` |
| `scope` | map | `InstrumentationScope.attributes` → `scope["key"]` |
| `attributes` | map | `Span.attributes` → `attributes["key"]` |

## Query Examples

### Scalars

Simple comparisons on top-level fields:

```cel
severity_number >= 17
severity_text == "ERROR"
service_name == "my-service"
event_name == "myapp.log"
body.contains("timeout")
metric_name == "cpu.usage"
value >= 100.0
name == "GetUser"
kind == 2
duration_ns >= 1_000_000_000
```

### Map bracket access

Access attributes by key using bracket notation. String and integer values
support equality and comparison:

```cel
resource["service.name"] == "my-service"
resource["host.name"].startsWith("k8s-node-")
attributes["http.status_code"] >= 400
attributes["custom.flags"] == 42
```

### List membership

For attribute values that are arrays, use `.exists()` with a predicate:

```cel
resource["custom.component"].exists(e, e == "processor-a")
scope["custom.channel"].exists(e, e == "control")
attributes["custom.thread"].exists(e, e == "worker-1")
```

List size and index access also work:

```cel
resource["custom.component"].size() >= 2
scope["custom.channel"].size() >= 3
resource["custom.component"][0] == "processor-a"
scope["custom.thread"][0] == "worker-1"
```

### Combined conditions

Logical AND (`&&`), OR (`||`), and negation (`!`) with parentheses:

```cel
severity_number >= 13 && service_name == "processor-a"
service_name == "processor-a" && body.contains("discard")
resource["custom.component"].exists(e, e == "processor-a") && severity_number >= 13
scope["custom.channel"].exists(e, e == "control") && scope["custom.thread"].exists(e, e == "worker-1")
attributes["custom.flags"] == 42 && attributes["custom.msg_type"] == 1
severity_number >= 13 || body.contains("timeout")
```

## CEL Functions

The `cel` crate provides these built-in functions:

| Function | Applies to | Example |
|----------|-----------|---------|
| `.contains(substr)` | string | `body.contains("error")` |
| `.startsWith(prefix)` | string | `service_name.startsWith("prod-")` |
| `.endsWith(suffix)` | string | `body.endsWith("failed")` |
| `.matches(regex)` | string | `body.matches(".*timeout.*")` |
| `.size()` | string, list, map | `scope["custom.channel"].size()` |
| `.exists(e, predicate)` | list | `.exists(e, e == "value")` |
| `has(variable)` | any | `has(attributes.error_message)` |

## Limitations

**`String`-only `.contains()`.** The `.contains()` function only operates on
strings. For list membership, use `.exists(e, e == "value")` instead. This is
a limitation of the `cel` crate v0.14.

```cel
# Works (string match)
body.contains("timeout")

# Does NOT work (list match)
resource["custom.component"].contains("processor-a")

# Use this instead
resource["custom.component"].exists(e, e == "processor-a")
```

**No `in` operator.** The CEL `in` operator is not yet available.

```cel
# Does NOT work
"processor-a" in resource["custom.component"]
```

**Error visibility in the TUI.** If a CEL expression fails to evaluate
(e.g., invalid syntax, type mismatch), the record is silently excluded.
Use the CLI `--cel` flag for immediate feedback on expression errors.
