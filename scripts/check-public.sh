#!/usr/bin/env bash
# Pre-push hygiene check for a public repo. Scans every tracked (and untracked,
# not ignored) file for secrets and personal data. Exit 1 on any hit.
#
#   scripts/check-public.sh
#   CHECK_PUBLIC_DENYLIST=~/private/flightydeck-terms.txt scripts/check-public.sh
#
# CHECK_PUBLIC_DENYLIST: optional file, one private term per line (names,
# booking refs, user ids). Matched case-insensitively. Never commit that file.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

# The one fake JWT allowed in the repo: the fixture token, whose payload has
# this synthetic subject.
FIXTURE_SUB='00000000-0000-4000-8000-000000000001'

failures=0
warnings=0
fail() { printf 'FAIL  %s\n' "$*"; failures=$((failures + 1)); }
warn() { printf 'WARN  %s\n' "$*"; warnings=$((warnings + 1)); }

files=()
while IFS= read -r -d '' f; do
  [[ -f "$f" ]] && files+=("$f")
done < <(git ls-files -z --cached --others --exclude-standard)

if [[ ${#files[@]} -eq 0 ]]; then
  echo "no files to scan"
  exit 0
fi

b64url_decode() {
  local s=${1//-/+}
  s=${s//_//}
  while (( ${#s} % 4 )); do s+='='; done
  printf '%s' "$s" | base64 -d 2>/dev/null || true
}

# 1. Database files must never be tracked.
while IFS= read -r f; do
  fail "database file in repo: $f"
done < <(printf '%s\n' "${files[@]}" | grep -E '\.(db|sqlite|sqlite3)(-journal|-wal|-shm)?$' || true)

# 2. JWT-shaped strings, except the fixture token.
while IFS= read -r hit; do
  loc=${hit%%:eyJ*}
  token=${hit#"$loc":}
  payload=$(printf '%s' "$token" | cut -d. -f2)
  if b64url_decode "$payload" | grep -q "\"sub\":\"$FIXTURE_SUB\""; then
    continue
  fi
  fail "JWT-shaped string: $loc"
done < <(grep -HnIoE 'eyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]*' -- "${files[@]}" 2>/dev/null || true)

# 3. Bearer tokens pasted from a request log. Built from parts so this script
# doesn't match itself.
bearer='Bearer'' ey'
while IFS= read -r hit; do
  fail "bearer token: ${hit%%:*}:$(cut -d: -f2 <<<"$hit")"
done < <(grep -HnIF "$bearer" -- "${files[@]}" 2>/dev/null || true)

# 4. Email addresses, except placeholders and the commit co-author address.
while IFS= read -r hit; do
  addr=${hit##*:}
  case "$addr" in
    *@example.com | *@example.org | *@example.net | noreply@anthropic.com | *@api.flightyapp.com) continue ;;
  esac
  fail "email address: $hit"
done < <(grep -HnIoE '[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}' -- "${files[@]}" 2>/dev/null || true)

# 5. UUIDs in tests/ that aren't the synthetic x0000000-0000-4000-8000-… shape
# may be real ids copied from a live database.
while IFS= read -r hit; do
  uuid=${hit##*:}
  [[ $(tr 'A-F' 'a-f' <<<"$uuid") =~ ^[0-9a-f]0000000-0000-4000-8000- ]] && continue
  warn "non-synthetic UUID in tests: $hit"
done < <(printf '%s\n' "${files[@]}" | grep '^tests/' | tr '\n' '\0' |
  xargs -0 grep -HnIoE '[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}' 2>/dev/null || true)

# 6. Owner's private terms.
if [[ -n "${CHECK_PUBLIC_DENYLIST:-}" ]]; then
  if [[ ! -r "$CHECK_PUBLIC_DENYLIST" ]]; then
    fail "CHECK_PUBLIC_DENYLIST is set but not readable: $CHECK_PUBLIC_DENYLIST"
  else
    while IFS= read -r term || [[ -n "$term" ]]; do
      term=${term%$'\r'}
      [[ -z "$term" || "$term" == \#* ]] && continue
      while IFS= read -r hit; do
        # Report the location only, never the term itself (CI logs are public).
        fail "denylisted term: $(cut -d: -f1-2 <<<"$hit")"
      done < <(grep -HnIiF -- "$term" "${files[@]}" 2>/dev/null || true)
    done <"$CHECK_PUBLIC_DENYLIST"
  fi
fi

echo "check-public: ${#files[@]} files, $failures failure(s), $warnings warning(s)"
if (( failures > 0 )); then
  echo "Fix the failures above before pushing." >&2
  exit 1
fi
