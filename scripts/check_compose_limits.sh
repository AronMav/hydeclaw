#!/usr/bin/env bash
# Phase 62 RES-06: lint docker-compose.yml — every service must have
# mem_limit AND cpus (not deploy.resources.limits — that's swarm-only).
#
# UNIX-ONLY: this script targets the Pi deploy host and Linux CI. It is
# NOT supported on Windows (dev workstation). The lint is enforced at
# deploy time on the Pi, not during local Windows dev. If you need to
# run it on Windows, use WSL2 or run it via the Pi over SSH.
set -euo pipefail

# ── OS gate ──────────────────────────────────────────────────────────
case "$(uname -s 2>/dev/null || echo unknown)" in
    Linux*|Darwin*|FreeBSD*|*BSD*) : ;;  # supported
    MINGW*|MSYS*|CYGWIN*|unknown)
        echo "skip: check_compose_limits.sh is Unix-only (deploy target = Pi/Linux)." >&2
        echo "      run under WSL2 or via the Pi over SSH. Exiting 0 for CI portability." >&2
        exit 0
        ;;
esac

COMPOSE_FILE="${1:-docker/docker-compose.yml}"
if [[ ! -f "$COMPOSE_FILE" ]]; then
    echo "error: compose file not found: $COMPOSE_FILE" >&2
    exit 2
fi

# ── CRLF normalization ───────────────────────────────────────────────
# Windows git checkouts can inject \r. Normalize to a tmpfile for all
# subsequent parsing. Trap cleanup; portable mktemp.
TMPFILE="$(mktemp -t check_compose_limits.XXXXXX)"
trap 'rm -f "$TMPFILE"' EXIT
tr -d '\r' < "$COMPOSE_FILE" > "$TMPFILE"

# ── Primary path: yq (mikefarah/yq v4+) ──────────────────────────────
# yq robustly parses YAML — immune to indentation quirks, nested
# structures, and anchor/alias tricks. Use it if available.
if command -v yq >/dev/null 2>&1; then
    # Extract service names via yq.
    mapfile -t services < <(yq -r '.services | keys | .[]' "$TMPFILE" 2>/dev/null || true)
    if [[ ${#services[@]} -eq 0 ]]; then
        echo "error: yq found no services in $COMPOSE_FILE" >&2
        exit 2
    fi
    missing_any=0
    for svc in "${services[@]}"; do
        mem=$(yq -r ".services.\"$svc\".mem_limit // \"\"" "$TMPFILE")
        cpus=$(yq -r ".services.\"$svc\".cpus // \"\"" "$TMPFILE")
        if [[ -z "$mem" ]]; then
            echo "MISSING mem_limit: $svc"
            missing_any=1
        fi
        if [[ -z "$cpus" ]]; then
            echo "MISSING cpus: $svc"
            missing_any=1
        fi
    done
    if [[ "$missing_any" -eq 1 ]]; then
        echo ""
        echo "FAIL: some services lack mem_limit or cpus. Phase 62 RES-06 requires both per service." >&2
        exit 1
    fi
    echo "OK: all ${#services[@]} services have mem_limit + cpus (via yq)"
    exit 0
fi

# ── Fallback: AWK over CRLF-normalized tmpfile ───────────────────────
# Limitations vs yq: only matches services written at the conventional
# 2-space indent under top-level "services:" with simple "name:" keys.
# Anchors/aliases/nested-includes not supported — install yq for those.
mapfile -t services < <(
    awk '
        /^services:/ { in_services=1; next }
        in_services && /^[a-zA-Z]/ { exit }
        in_services && /^  [a-zA-Z][a-zA-Z0-9_.-]*:$/ {
            gsub(/^[[:space:]]+|:$/, "", $0); print
        }
    ' "$TMPFILE"
)

if [[ ${#services[@]} -eq 0 ]]; then
    echo "error: no services found in $COMPOSE_FILE (AWK fallback — install yq for better parsing)" >&2
    exit 2
fi

missing_any=0
for svc in "${services[@]}"; do
    # Find the service block: from "^  $svc:$" to the next "^  \S" at 2-space indent.
    block=$(awk -v s="  $svc:" '
        $0 == s { in_block=1; print; next }
        in_block && /^  [a-zA-Z]/ { exit }
        in_block { print }
    ' "$TMPFILE")

    has_mem=$(echo "$block" | grep -c "^    mem_limit:" || true)
    has_cpus=$(echo "$block" | grep -c "^    cpus:" || true)

    if [[ "$has_mem" -lt 1 ]]; then
        echo "MISSING mem_limit: $svc"
        missing_any=1
    fi
    if [[ "$has_cpus" -lt 1 ]]; then
        echo "MISSING cpus: $svc"
        missing_any=1
    fi
done

if [[ "$missing_any" -eq 1 ]]; then
    echo ""
    echo "FAIL: some services lack mem_limit or cpus. Phase 62 RES-06 requires both per service." >&2
    echo "      (AWK fallback — for nested/non-standard compose shapes, install yq.)" >&2
    exit 1
fi

echo "OK: all ${#services[@]} services have mem_limit + cpus (AWK fallback)"
exit 0
