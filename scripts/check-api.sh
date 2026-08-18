#!/usr/bin/env bash
# Exercise the running server over real HTTP.
#
# Clears the users table first. "The first account becomes the administrator"
# can only be tested against an empty one, and without this the script passes
# once and then fails on every later run for a reason that has nothing to do
# with the server. Dev database only -- it truncates.
set -u
API=http://127.0.0.1:7878/api
TAG=$RANDOM$RANDOM
fail=0

PSQL='/c/Program Files/PostgreSQL/17/bin/psql.exe'
ENVFILE="$(cd "$(dirname "$0")/.." && pwd)/.env"
URL=$(sed -n 's/^DATABASE_URL=//p' "$ENVFILE")
if [ -n "$URL" ]; then
  # Everything else cascades from users.
  PGPASSWORD=$(echo "$URL" | sed -n 's|.*://[^:]*:\([^@]*\)@.*|\1|p') \
    "$PSQL" -h 127.0.0.1 -p 5432 \
      -U "$(echo "$URL" | sed -n 's|.*://\([^:]*\):.*|\1|p')" \
      -d "$(echo "$URL" | sed -n 's|.*/\([^/]*\)$|\1|p')" \
      --no-password -q -c 'TRUNCATE users CASCADE;' >/dev/null 2>&1 \
    && echo "(users cleared for a repeatable run)" \
    || echo "(could not clear users; the admin checks may not be meaningful)"
fi

# code <method> <path> [token] [body]  -> prints the HTTP status only
code() {
  local m=$1 p=$2 t=${3:-} b=${4:-}
  local args=(-s -o /dev/null -w '%{http_code}' -X "$m" "$API$p")
  [ -n "$t" ] && args+=(-H "Authorization: Bearer $t")
  [ -n "$b" ] && args+=(-H 'Content-Type: application/json' -d "$b")
  curl "${args[@]}"
}
body() {
  local m=$1 p=$2 t=${3:-} b=${4:-}
  local args=(-s -X "$m" "$API$p")
  [ -n "$t" ] && args+=(-H "Authorization: Bearer $t")
  [ -n "$b" ] && args+=(-H 'Content-Type: application/json' -d "$b")
  curl "${args[@]}"
}
expect() { # expect <label> <want> <got>
  if [ "$2" = "$3" ]; then printf '  ok   %-46s %s\n' "$1" "$3"
  else printf '  FAIL %-46s want %s got %s\n' "$1" "$2" "$3"; fail=1; fi
}

echo "--- open endpoints ---"
expect "health is reachable unauthenticated" 200 "$(code GET /health)"
expect "setup-state is reachable"            200 "$(code GET /setup-state)"

echo
echo "--- everything else needs a token ---"
for p in /me /books /settings /ollama/status; do
  expect "GET $p without a token" 401 "$(code GET "$p")"
done
expect "a made-up token"        401 "$(code GET /books "$(printf '0%.0s' {1..64})")"
expect "a malformed header"     401 "$(curl -s -o /dev/null -w '%{http_code}' -H 'Authorization: nonsense' $API/books)"

echo
echo "--- registration ---"
A=$(body POST /register '' "{\"email\":\"ann-$TAG@example.test\",\"password\":\"a long enough one\"}")
TA=$(echo "$A" | sed -n 's/.*"token":"\([^"]*\)".*/\1/p')

# Ann got in because the server was empty. Registration closes behind her, so
# a second account needs her to open it -- which is the property the section
# further down tests properly.
OPEN=$(printf '{"ollama_host":"http://127.0.0.1:11434","ocr_model":"glm-ocr","text_model":"qwen3:4b","vision_model":"qwen3-vl:4b","keep_alive":"30m","open_registration":true}')
code PUT /settings "$TA" "$OPEN" >/dev/null
B=$(body POST /register '' "{\"email\":\"ben-$TAG@example.test\",\"password\":\"a long enough one\"}")
TB=$(echo "$B" | sed -n 's/.*"token":"\([^"]*\)".*/\1/p')
code PUT /settings "$TA" "$(echo "$OPEN" | sed 's/"open_registration":true/"open_registration":false/')" >/dev/null
[ -n "$TA" ] && echo "  ok   ann registered and signed in" || { echo "  FAIL ann: $A"; fail=1; }
[ -n "$TB" ] && echo "  ok   ben registered and signed in" || { echo "  FAIL ben: $B"; fail=1; }

echo "$A" | grep -q '"is_admin":true'  && echo "  ok   first account is the administrator" || { echo "  FAIL ann is not admin"; fail=1; }
echo "$B" | grep -q '"is_admin":false' && echo "  ok   second account is not"             || { echo "  FAIL ben is admin"; fail=1; }
echo "$A" | grep -q 'password' && { echo "  FAIL the response echoes a password field"; fail=1; } || echo "  ok   no password field in the response"

expect "a short password"       400 "$(code POST /register '' "{\"email\":\"x-$TAG@example.test\",\"password\":\"short\"}")"
expect "a duplicate address"    400 "$(code POST /register '' "{\"email\":\"ann-$TAG@example.test\",\"password\":\"a long enough one\"}")"
expect "sign in, wrong password" 400 "$(code POST /sign-in '' "{\"email\":\"ann-$TAG@example.test\",\"password\":\"nope\"}")"

echo
echo "--- libraries are separate ---"
BOOK=$(body POST /books "$TA" '{"title":"Systematic Theology","author":"Charles Hodge","era":"victorian"}' | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
echo "  ok   ann created book $BOOK"
expect "ann sees 1 book"  1 "$(body GET /books "$TA" | grep -o '"id":' | wc -l | tr -d ' ')"
expect "ben sees 0 books" 0 "$(body GET /books "$TB" | grep -o '"id":' | wc -l | tr -d ' ')"

expect "ben GET ann's book"        404 "$(code GET "/books/$BOOK" "$TB")"
expect "ben GET its stats"         404 "$(code GET "/books/$BOOK/stats" "$TB")"
expect "ben GET its pages"         200 "$(code GET "/books/$BOOK/pages" "$TB")"
expect "  ...and they are empty"   0   "$(body GET "/books/$BOOK/pages" "$TB" | grep -o '"id":' | wc -l | tr -d ' ')"
expect "ben DELETE ann's book"     404 "$(code DELETE "/books/$BOOK" "$TB")"
expect "ann's book survived"       200 "$(code GET "/books/$BOOK" "$TA")"

echo
echo "--- admin-only settings ---"
expect "ann reads settings"        200 "$(code GET /settings "$TA")"
expect "ben reads settings"        200 "$(code GET /settings "$TB")"
S='{"ollama_host":"http://127.0.0.1:11434","ocr_model":"glm-ocr","text_model":"qwen3:4b","vision_model":"qwen3-vl:4b","keep_alive":"30m"}'
expect "ben writes settings (SSRF vector)" 404 "$(code PUT /settings "$TB" "$S")"
expect "ann writes settings"               200 "$(code PUT /settings "$TA" "$S")"
expect "ben probes an arbitrary host"      404 "$(code GET '/ollama/models?host=http://169.254.169.254' "$TB")"

# Store a real key, then try to read it back. The response carries a boolean
# saying whether one is set, so grepping for the field name proves nothing --
# it is the secret itself that must not come back.
SECRET="sk-canary-$TAG-do-not-leak"
WITHKEY=$(printf '{"ollama_host":"http://127.0.0.1:11434","ollama_api_key":"%s","ocr_model":"glm-ocr","text_model":"qwen3:4b","vision_model":"qwen3-vl:4b","keep_alive":"30m"}' "$SECRET")
expect "ann stores an API key"             200 "$(code PUT /settings "$TA" "$WITHKEY")"

for who in TA TB; do
  got=$(body GET /settings "${!who}")
  if echo "$got" | grep -q "$SECRET"; then
    echo "  FAIL the API key is readable by \$$who"; fail=1
  else
    echo "  ok   the key value is not readable by \$$who"
  fi
done
body GET /settings "$TA" | grep -q '"has_api_key":true' && echo "  ok   but the client is told one is set" || { echo "  FAIL has_api_key did not update"; fail=1; }

# Omitting the field must leave the stored key alone, or a client that never
# sees it cannot save any other setting without wiping it.
expect "saving without the key field"      200 "$(code PUT /settings "$TA" "$S")"
body GET /settings "$TA" | grep -q '"has_api_key":true' && echo "  ok   the key survived a save that omitted it" || { echo "  FAIL an omitted key field cleared the key"; fail=1; }
expect "clearing it explicitly"            200 "$(code PUT /settings "$TA" "$(printf '{"ollama_host":"http://127.0.0.1:11434","ollama_api_key":"","ocr_model":"glm-ocr","text_model":"qwen3:4b","vision_model":"qwen3-vl:4b","keep_alive":"30m"}')")"
body GET /settings "$TA" | grep -q '"has_api_key":false' && echo "  ok   an empty string clears it" || { echo "  FAIL empty string did not clear the key"; fail=1; }

echo
echo "--- registration is closed once the library exists ---"
# The bootstrap case is above: ann registered on an empty server. Ben got in
# only because that same request had not yet been made when he tried. From
# here on a third party must be refused, or a server on a network is an open
# signup form forever.
expect "a stranger registers"     400 "$(code POST /register '' "{\"email\":\"gate-$TAG@example.test\",\"password\":\"a long enough one\"}")"
body GET /setup-state | grep -q '"registration_open":false' && echo "  ok   and the sign-in screen is told so" || { echo "  FAIL setup-state still says registration is open"; fail=1; }

expect "ben opens registration"   404 "$(code PUT /settings "$TB" "$OPEN")"
expect "ann opens registration"   200 "$(code PUT /settings "$TA" "$OPEN")"
expect "now a stranger can"       200 "$(code POST /register '' "{\"email\":\"gate-$TAG@example.test\",\"password\":\"a long enough one\"}")"
body POST /register '' "{\"email\":\"gate2-$TAG@example.test\",\"password\":\"a long enough one\"}" | grep -q '"is_admin":false' && echo "  ok   but not as an administrator" || { echo "  FAIL a later account got admin"; fail=1; }

CLOSED=$(echo "$OPEN" | sed 's/"open_registration":true/"open_registration":false/')
expect "ann closes it again"      200 "$(code PUT /settings "$TA" "$CLOSED")"
expect "and it is shut"           400 "$(code POST /register '' "{\"email\":\"gate3-$TAG@example.test\",\"password\":\"a long enough one\"}")"

# A client that predates this setting must not turn it on by omission.
expect "saving without the field" 200 "$(code PUT /settings "$TA" "$S")"
body GET /settings "$TA" | grep -q '"open_registration":false' && echo "  ok   an omitted field leaves it closed" || { echo "  FAIL omitting the field opened registration"; fail=1; }


echo "--- guessing is slowed down ---"
# The legitimate reader is checked FIRST, on purpose. Every request here comes
# from 127.0.0.1, so the guessing below fills the shared per-source counter --
# and running these afterwards would fail for that reason rather than for
# anything wrong with the server.
FRESH="fresh-$TAG@example.test"
code PUT /settings "$TA" "$OPEN" >/dev/null
body POST /register '' "{\"email\":\"$FRESH\",\"password\":\"a long enough one\"}" >/dev/null
code PUT /settings "$TA" "$CLOSED" >/dev/null
for i in 1 2 3 4; do code POST /sign-in '' "{\"email\":\"$FRESH\",\"password\":\"wrong\"}" >/dev/null; done
expect "the right password still works"      200 "$(code POST /sign-in '' "{\"email\":\"$FRESH\",\"password\":\"a long enough one\"}")"
expect "and signing in cleared the count"    400 "$(code POST /sign-in '' "{\"email\":\"$FRESH\",\"password\":\"wrong\"}")"

# Five free against one account, then a wait that doubles. The free attempts
# matter: someone mistyping their own password is far more common than an
# attacker, and should not be punished for it.
GUESS="guess-$TAG@example.test"
wrong() { code POST /sign-in '' "{\"email\":\"$GUESS\",\"password\":\"wrong\"}"; }

allowed=0
for i in 1 2 3 4 5; do
  [ "$(wrong)" = "400" ] && allowed=$((allowed+1))
done
expect "the first five are answered normally" 5 "$allowed"
expect "the sixth is refused"                429 "$(wrong)"

# The header has to be there, or a client has nothing to obey.
ra=$(curl -s -D- -o /dev/null -X POST "$API/sign-in" -H 'Content-Type: application/json' \
      -d "{\"email\":\"$GUESS\",\"password\":\"wrong\"}" | grep -i '^retry-after:' | tr -d '\r' | awk '{print $2}')
[ -n "$ra" ] && echo "  ok   it says how long to wait (Retry-After: $ra)" || { echo "  FAIL no Retry-After header"; fail=1; }

# A refused attempt must not reach argon2, or the throttle would cost the
# server the very work it exists to avoid.
t0=$(date +%s%N); wrong >/dev/null; t1=$(date +%s%N)
blocked_ms=$(( (t1-t0)/1000000 ))
[ "$blocked_ms" -lt 200 ] && echo "  ok   a blocked attempt is cheap (${blocked_ms}ms, no password hashing)" \
  || { echo "  FAIL a blocked attempt still cost ${blocked_ms}ms"; fail=1; }
echo
echo "--- dictionary ---"
NICE=$(body POST /dictionary/look-up "$TA" '{"word":"nice"}')
echo "$NICE" | grep -qi 'foolish\|silly' && echo "  ok   'nice' returns its 1913 sense" || { echo "  FAIL nice: ${NICE:0:120}"; fail=1; }
expect "lookup needs a token"      401 "$(code POST /dictionary/look-up '' '{"word":"nice"}')"

echo
echo "--- sign out ---"
expect "sign out"                  204 "$(code POST /sign-out "$TB")"
expect "the token is dead"         401 "$(code GET /me "$TB")"
expect "ann is unaffected"         200 "$(code GET /me "$TA")"

echo
[ $fail -eq 0 ] && echo "All API checks passed." || echo "SOME CHECKS FAILED."
exit $fail
