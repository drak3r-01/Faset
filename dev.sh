#!/usr/bin/env sh
# Thin wrapper: all logic lives in `cargo xtask`. Usage: ./dev.sh <command> [args]
exec docker compose run --rm dev cargo xtask "$@"
