#!/usr/bin/env bash
# Generate the TEST-ONLY RSA keys for fake-google. Idempotent: regenerates if
# missing or forced with --force. These keys are NOT secrets: they are committed
# so fake-google (test infrastructure only) can sign deterministic RS256 JWTs
# without pulling in the `rsa` crate (which carries an open RustSec advisory).
#
# The matching public `n`/`e` constants live in fake-google/src/oidc.rs
# (JWKS_PUBLIC_N, JWKS_PUBLIC_E). If you regenerate the keys, re-extract n/e:
#   openssl rsa -in <key> -pubout -outform DER | \
#     python3 -c "import base64,sys; der=sys.stdin.buffer.read(); \
#       # n is the first INTEGER inside the inner RSAPublicKey, e the second"

set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/backend/crates/testkit/fixtures/keys"
FORCE="${1:-}"

mkdir -p "$DIR"

gen() {
    local out="$1"
    if [[ -f "$out" && "$FORCE" != "--force" ]]; then
        echo "exists: $out"
        return
    fi
    openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out "$out"
    chmod 600 "$out"
    echo "wrote: $out"
}

gen "$DIR/TEST-ONLY-fake-google-rs256.pem"
gen "$DIR/TEST-ONLY-fake-google-rs256-other.pem"
