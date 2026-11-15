# Thin wrapper: all logic lives in `cargo xtask`. Usage: .\dev.ps1 <command> [args]
docker compose run --rm dev cargo xtask @args
exit $LASTEXITCODE
