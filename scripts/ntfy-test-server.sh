#!/usr/bin/env bash
# Local ntfy server for the e2e tests (tests/e2e_ntfy.rs) on 127.0.0.1:18080.
# Usage: scripts/ntfy-test-server.sh start|stop|restart [--no-cache]
set -euo pipefail

NAME=ntfy-native-test
VOLUME=ntfy-native-test-cache
IMAGE=binwiederhier/ntfy
ADDR=127.0.0.1:18080

start() {
    local args=(serve --listen-http :80 --keepalive-interval 5s)
    if [[ "${1:-}" != "--no-cache" ]]; then
        args+=(--cache-file /var/cache/ntfy/cache.db)
    fi
    docker run -d --name "$NAME" -p "$ADDR:80" -v "$VOLUME:/var/cache/ntfy" "$IMAGE" "${args[@]}" >/dev/null
    for _ in $(seq 1 50); do
        if curl -fsS "http://$ADDR/v1/health" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.2
    done
    echo "ntfy did not become healthy" >&2
    docker logs "$NAME" >&2 || true
    return 1
}

stop_container() {
    docker rm -f "$NAME" >/dev/null 2>&1 || true
}

case "${1:-}" in
    start) stop_container; start "${2:-}" ;;
    restart) stop_container; start "${2:-}" ;;
    stop) stop_container; docker volume rm "$VOLUME" >/dev/null 2>&1 || true ;;
    *) echo "usage: $0 start|stop|restart [--no-cache]" >&2; exit 2 ;;
esac
