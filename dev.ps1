# Thin wrapper: all logic lives in `cargo xtask`. Usage: .\dev.ps1 <command> [args]
# `run-server [dedicated args]` builds and starts the dedicated server container
# with UDP port 7777 published (console on stdin, Ctrl+C stops it cleanly).
if ($args.Count -gt 0 -and $args[0] -eq 'run-server') {
    $rest = @($args | Select-Object -Skip 1)
    docker compose build server
    if ($LASTEXITCODE -eq 0) { docker compose run --rm --service-ports server @rest }
    exit $LASTEXITCODE
}
docker compose run --rm dev cargo xtask @args
exit $LASTEXITCODE
