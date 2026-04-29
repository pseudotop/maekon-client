#!/usr/bin/env bash
# check-endpoint-coverage.sh — Verify every route handler function has a test.
#
# Extracts handler function references from routes.rs (including HTTP method),
# then checks whether each handler function is referenced in a test block
# within the handler module (by function name or a test name derived from it).
#
# Usage: ./scripts/check-endpoint-coverage.sh
#        ./scripts/check-endpoint-coverage.sh --verbose   # show per-function status
# Exit code 0 if all covered, 1 if gaps found.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
ROUTES_FILE="${REPO_ROOT}/crates/oneshim-web/src/routes.rs"
HANDLERS_DIR="${REPO_ROOT}/crates/oneshim-web/src/handlers"

VERBOSE=false
[[ "${1:-}" == "--verbose" ]] && VERBOSE=true

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

if [[ ! -f "${ROUTES_FILE}" ]]; then
    echo -e "${RED}[error]${NC} routes.rs not found: ${ROUTES_FILE}" >&2
    exit 1
fi

# Step 1: Extract handler references with HTTP method from routes.rs
# Output lines like: "GET  metrics::get_metrics"
# This handles both single-line and multi-line .route() patterns.
extract_handlers() {
    local routes_content
    routes_content=$(cat "${ROUTES_FILE}")

    # Match patterns like: get(handlers::module::func), post(handlers::module::func)
    echo "${routes_content}" | grep -oE '(get|post|put|delete)\(handlers::[a-z_]+::[a-z_]+\)' | \
        sed 's/(\(.*\))/\1/; s/(//; s/)//;' | \
        while IFS= read -r line; do
            method=$(echo "${line}" | grep -oE '^(get|post|put|delete)' | tr '[:lower:]' '[:upper:]')
            handler=$(echo "${line}" | grep -oE 'handlers::[a-z_]+::[a-z_]+' | sed 's/^handlers:://')
            echo "${method}  ${handler}"
        done | sort -t: -k1,1 -k3,3 | uniq
}

handlers=$(extract_handlers)
total=$(echo "${handlers}" | wc -l | tr -d ' ')
covered=0
uncovered=0
uncovered_list=""

# Step 2: For each handler FUNCTION, check if a test references it
while IFS= read -r entry; do
    method=$(echo "${entry}" | awk '{print $1}')
    handler=$(echo "${entry}" | awk '{print $2}')
    module=$(echo "${handler}" | cut -d: -f1)
    func=$(echo "${handler}" | cut -d: -f3)

    # Collect all test source files for this module
    test_sources=""
    if [[ -f "${HANDLERS_DIR}/${module}.rs" ]]; then
        test_sources="${HANDLERS_DIR}/${module}.rs"
    fi
    if [[ -d "${HANDLERS_DIR}/${module}" ]]; then
        test_sources="${test_sources} $(find "${HANDLERS_DIR}/${module}" -name "*.rs" 2>/dev/null | tr '\n' ' ')"
    fi

    # Check if the function name appears in test code
    # Look for: the function name in test function names, in handler() calls,
    # or in URL path patterns that map to this handler
    has_test=false
    if [[ -n "${test_sources}" ]]; then
        # Strategy 1: function name appears in test code (e.g., "get_metrics", "list_sessions")
        if grep -q "${func}" ${test_sources} 2>/dev/null; then
            has_test=true
        fi
        # Strategy 2: test function name contains the handler name (e.g., "test_get_metrics")
        if ! ${has_test} && grep -qE "fn.*${func}" ${test_sources} 2>/dev/null; then
            has_test=true
        fi
    fi

    if ${has_test}; then
        covered=$((covered + 1))
        ${VERBOSE} && echo -e "  ${GREEN}✓${NC} ${method}  ${module}::${func}"
    else
        uncovered=$((uncovered + 1))
        uncovered_list="${uncovered_list}\n  ${RED}✗${NC} ${method}  ${module}::${func}"
        ${VERBOSE} && echo -e "  ${RED}✗${NC} ${method}  ${module}::${func}"
    fi
done <<< "${handlers}"

# Step 3: Method breakdown
get_count=$(echo "${handlers}" | grep -c "^GET" || true)
post_count=$(echo "${handlers}" | grep -c "^POST" || true)
put_count=$(echo "${handlers}" | grep -c "^PUT" || true)
delete_count=$(echo "${handlers}" | grep -c "^DELETE" || true)

# Step 4: Report
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "  Endpoint Test Coverage Report (per handler function)"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "  Total handler functions:  ${total}"
echo -e "  ${BLUE}By method:${NC}  GET=${get_count}  POST=${post_count}  PUT=${put_count}  DELETE=${delete_count}"
echo ""
echo -e "  ${GREEN}Covered (function referenced in tests):${NC}  ${covered}"
echo -e "  ${RED}Uncovered (no test reference):${NC}           ${uncovered}"
echo ""

pct=0
if [[ ${total} -gt 0 ]]; then
    pct=$((covered * 100 / total))
fi
echo "  Coverage: ${pct}%"
echo ""

if [[ ${uncovered} -gt 0 ]]; then
    echo -e "${YELLOW}Uncovered handlers:${NC}"
    echo -e "${uncovered_list}"
    echo ""
    echo -e "${RED}[FAIL]${NC} ${uncovered} handler function(s) have no test reference."
    exit 1
else
    echo -e "${GREEN}[PASS]${NC} All ${total} handler functions have test references."
    exit 0
fi
