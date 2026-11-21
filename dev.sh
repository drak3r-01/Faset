#!/usr/bin/env sh
# Thin wrapper: all logic lives in `cargo xtask`. Usage: ./dev.sh <command> [args]
# `run-server [dedicated args]` starts the dedicated server container (UDP 7777).
if [ "$1" = "run-server" ]; then
    shift
    docker compose build server || exit $?
    exec docker compose run --rm --service-ports server "$@"
fi
exec docker compose run --rm dev cargo xtask "$@"
