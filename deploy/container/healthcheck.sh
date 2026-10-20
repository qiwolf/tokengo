#!/bin/sh
set -eu
# /readyz uses HTTP 200 even when degraded; check the JSON status as well.
curl --fail --silent --show-error --max-time 4 http://127.0.0.1:3160/readyz \
    | grep -Eq '"status"[[:space:]]*:[[:space:]]*"ok"'
