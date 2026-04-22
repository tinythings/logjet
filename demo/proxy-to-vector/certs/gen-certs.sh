#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
CA_PEM="$SCRIPT_DIR/ca.pem"
CA_KEY="$SCRIPT_DIR/ca.key"
SERVER_PEM="$SCRIPT_DIR/vector.pem"
SERVER_KEY="$SCRIPT_DIR/vector.key"

if [ -f "$CA_PEM" ] && [ -f "$CA_KEY" ] && [ -f "$SERVER_PEM" ] && [ -f "$SERVER_KEY" ]; then
    exit 0
fi

if ! command -v openssl >/dev/null 2>&1; then
    echo "missing openssl"
    exit 1
fi

tmpdir=$(mktemp -d)
cleanup() {
    rm -rf "$tmpdir"
}
trap cleanup EXIT INT TERM

cat >"$tmpdir/ca.cnf" <<'EOF'
[req]
distinguished_name = dn
x509_extensions = v3_ca
prompt = no

[dn]
CN = proxy-to-vector demo CA

[v3_ca]
basicConstraints = critical,CA:TRUE
keyUsage = critical,keyCertSign,cRLSign
subjectKeyIdentifier = hash
authorityKeyIdentifier = keyid:always,issuer
EOF

cat >"$tmpdir/vector.cnf" <<'EOF'
[req]
distinguished_name = dn
req_extensions = v3_req
prompt = no

[dn]
CN = vector.demo.logjet

[v3_req]
basicConstraints = critical,CA:FALSE
keyUsage = critical,digitalSignature,keyEncipherment
extendedKeyUsage = serverAuth
subjectAltName = @alt_names

[alt_names]
DNS.1 = vector.demo.logjet
IP.1 = 127.0.0.1
EOF

openssl req -x509 -newkey rsa:2048 -nodes -keyout "$CA_KEY" -out "$CA_PEM" -days 3650 -config "$tmpdir/ca.cnf" >/dev/null 2>&1
openssl req -newkey rsa:2048 -nodes -keyout "$SERVER_KEY" -out "$tmpdir/vector.csr" -config "$tmpdir/vector.cnf" >/dev/null 2>&1
openssl x509 -req -in "$tmpdir/vector.csr" -CA "$CA_PEM" -CAkey "$CA_KEY" -CAcreateserial -out "$SERVER_PEM" -days 3650 -extensions v3_req -extfile "$tmpdir/vector.cnf" >/dev/null 2>&1
