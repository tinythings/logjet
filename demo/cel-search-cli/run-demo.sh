#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT_DIR=$(CDPATH= cd -- "$SCRIPT_DIR/../.." && pwd)
GENERATOR="$ROOT_DIR/target/debug/cel-search-cli-logjet-generator"
LJX="$ROOT_DIR/target/debug/ljx"
CONFIG="$SCRIPT_DIR/cel-search-cli.conf"

if [ ! -x "$GENERATOR" ] || [ ! -x "$LJX" ]; then
    echo "missing demo binaries"
    echo "build them first with: cargo build -p otlp-demo --bin cel-search-cli-logjet-generator -p ljx"
    exit 1
fi

. "$CONFIG"

cd "$SCRIPT_DIR"
mkdir -p logs
rm -f "$OUTPUT_FILE"

echo "generating $COUNT diverse log records into $OUTPUT_FILE"
"$GENERATOR" "$OUTPUT_FILE" "$COUNT"

echo
printf '\033[1;38;5;200m=== CEL Search Demos ===\033[0m\n'
echo "File: $OUTPUT_FILE ($COUNT records)"
echo

run_query() {
    local desc="$1"
    shift
    printf '\033[1;38;5;253m%s\033[0m\n' "$desc"
    printf '  \033[38;5;43mcmd:\033[0m ljx count %s %s\n' "$OUTPUT_FILE" "$*"
    printf '  \033[38;5;43mres:\033[0m '
    "$LJX" count "$OUTPUT_FILE" "$@"
    echo
}

echo "--- Severity queries ---"

run_query "ERROR or FATAL (severity_number >= 17)" --cel 'severity_number >= 17'

run_query "INFO only (severity_text == INFO)" --cel 'severity_text == "INFO"'

run_query "WARN and above" --cel 'severity_number >= 13'

run_query "DEBUG only" --cel 'severity_text == "DEBUG"'

echo "--- Body substring searches ---"

run_query "body contains timeout (payment timeouts)" --cel 'body.contains("timeout")'

run_query "body contains failed (payment or auth failures)" --cel 'body.contains("failed")'

run_query "body contains cache (cache hits and misses)" --cel 'body.contains("cache")'

run_query "body contains login (auth events)" --cel 'body.contains("login")'

echo "--- Service name filtering ---"

run_query "payment-worker only" --cel 'service_name == "payment-worker"'

run_query "auth-gateway only" --cel 'service_name == "auth-gateway"'

run_query "core-api or cache-layer" --cel 'service_name == "core-api" || service_name == "cache-layer"'

echo "--- Attribute access ---"

run_query "HTTP 5xx status codes" --cel 'attributes["http.status_code"] >= 500'

run_query "HTTP 404 status" --cel 'attributes["http.status_code"] == 404'

run_query "has error.code attribute (non-empty)" --cel 'attributes["error.code"] != ""'

run_query "specific error code AUTH_FAILED" --cel 'attributes["error.code"] == "AUTH_FAILED"'

echo "--- Resource attribute access ---"

run_query "eu-west-1 region" --cel 'resource["deploy.region"] == "eu-west-1"'

run_query "staging environment" --cel 'resource["deploy.env"] == "staging"'

echo "--- Event name filtering ---"

run_query "auth login events" --cel 'event_name == "auth.login"'

run_query "http request events" --cel 'event_name == "http.request"'

echo "--- Combined conditions ---"

run_query "auth failed (body + event_name)" --cel 'body.contains("failed") && event_name == "auth.login"'

run_query "cache miss on cache-layer" --cel 'body.contains("miss") && service_name == "cache-layer"'

run_query "ERROR with consumer_lag critical" --cel 'severity_number >= 17 && body.contains("critical")'

echo "--- CEL + other filters (AND semantics) ---"

run_query "CEL + --grep (WARN+ with 'login' in raw bytes)" --cel 'severity_number >= 13' --grep login

run_query "CEL + --fixed-string (ERROR+ with 'email' exact match)" --cel 'severity_number >= 17' --fixed-string email

echo "--- Record preview (NDJSON) ---"

printf '\033[1;38;5;253m%s\033[0m\n' "CEL search with NDJSON output (matching records)"
printf '  \033[38;5;43mcmd:\033[0m ljx %s --cel '\''severity_number >= 17'\'' --format ndjson\n' "$OUTPUT_FILE"
printf '  \033[38;5;43mres:\033[0m\n'
"$LJX" "$OUTPUT_FILE" --cel 'severity_number >= 17' --format ndjson 2>/dev/null | head -5
echo "  ..."

echo
echo "--- Time range queries ---"

T1=1773000000000000000
T2=1773009000000000000

run_query "records in first 100 records (time window)" --ts-min "$T1" --ts-max "$T2" --cel 'event_name == "http.request"'
run_query "all records in first 100 records" --ts-min "$T1" --ts-max "$T2"

echo
echo "--- Multiple CEL expressions (AND semantics) ---"

run_query "two --cel flags: cache AND not hit (misses)" --cel 'body.contains("cache")' --cel '!body.contains("hit")'

run_query "two --cel flags: ERROR severity + has error code" --cel 'severity_number >= 17' --cel 'attributes["error.code"] != ""'

echo
printf '\033[1;38;5;200mDone. Try TUI mode: %s view %s\033[0m\n' "$LJX" "$OUTPUT_FILE"
