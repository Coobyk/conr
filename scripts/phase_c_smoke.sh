#!/usr/bin/env bash
set -euo pipefail

export DATABASE_URL="${DATABASE_URL:-postgres://localhost/conr}"
export CONR_PORT="${CONR_PORT:-18084}"
BASE="http://127.0.0.1:${CONR_PORT}"

fuser -k "${CONR_PORT}/tcp" 2>/dev/null || true
cargo build -p conr-server -q
cargo run -p conr-server >/tmp/conr-phasec.log 2>&1 &
SPID=$!
trap 'kill $SPID 2>/dev/null || true' EXIT

for _ in $(seq 1 40); do
  if curl -sf "$BASE/health" >/dev/null; then
    break
  fi
  sleep 0.25
done
curl -sf "$BASE/health" >/dev/null

H1="za$(date +%s | tail -c 6)"
H2="zb$(date +%s | tail -c 6)"

R1=$(curl -s -X POST "$BASE/auth/register" -H 'content-type: application/json' -d "{\"handle\":\"$H1\"}")
R2=$(curl -s -X POST "$BASE/auth/register" -H 'content-type: application/json' -d "{\"handle\":\"$H2\"}")
AT1=$(printf '%s' "$R1" | python3 -c 'import sys,json; print(json.load(sys.stdin)["access_token"])')
AT2=$(printf '%s' "$R2" | python3 -c 'import sys,json; print(json.load(sys.stdin)["access_token"])')
echo "users: $H1 $H2"

P1=$(curl -s -X POST "$BASE/posts" -H "authorization: Bearer $AT1" -H 'content-type: application/json' \
  -d "{\"content\":\"hello #shipit @$H2 ready\"}")
PID=$(printf '%s' "$P1" | python3 -c 'import sys,json; print(json.load(sys.stdin)["id"])')
echo "post: $PID"

python3 - "$AT2" "$BASE" "$H1" <<'PY'
import json,sys,urllib.request
at,base,h1=sys.argv[1:4]
req=urllib.request.Request(base+f"/users/{h1}/follow", method="POST", headers={"authorization":f"Bearer {at}"})
d=json.load(urllib.request.urlopen(req))
assert d.get("followed_by_me") is True, d
assert d.get("followers") >= 1, d
print("follow: ok")
PY

python3 - "$AT2" "$BASE" "$PID" <<'PY'
import json,sys,urllib.request
at,base,pid=sys.argv[1:4]
req=urllib.request.Request(base+"/feed?filter=following", headers={"authorization":f"Bearer {at}"})
d=json.load(urllib.request.urlopen(req))
assert "error" not in d, d
assert any(p["id"]==pid for p in d["posts"]), d
print("following feed: ok")
PY

python3 - "$AT1" "$BASE" <<'PY'
import json,sys,urllib.request,urllib.parse
at,base=sys.argv[1:3]
q=urllib.parse.urlencode({"q":"shipit"})
req=urllib.request.Request(base+"/search?"+q, headers={"authorization":f"Bearer {at}"})
d=json.load(urllib.request.urlopen(req))
assert "error" not in d, d
assert len(d["posts"]) >= 1, d
print("search fts: ok", len(d["posts"]), "tags", d.get("tags"))
PY

python3 - "$AT1" "$BASE" <<'PY'
import json,sys,urllib.request,urllib.parse
at,base=sys.argv[1:3]
q=urllib.parse.urlencode({"q":"#shipit"})
req=urllib.request.Request(base+"/search?"+q, headers={"authorization":f"Bearer {at}"})
d=json.load(urllib.request.urlopen(req))
assert "error" not in d, d
assert len(d["posts"]) >= 1, d
print("search tag: ok", len(d["posts"]))
PY

python3 - "$AT2" "$BASE" <<'PY'
import json,sys,urllib.request
at,base=sys.argv[1:3]
req=urllib.request.Request(base+"/notifications", headers={"authorization":f"Bearer {at}"})
d=json.load(urllib.request.urlopen(req))
assert "error" not in d, d
kinds={n["kind"] for n in d["notifications"]}
assert "mention" in kinds, d
print("bob notifs: ok", sorted(kinds), "unread", d["unread"])
PY

python3 - "$AT1" "$BASE" "$PID" <<'PY'
import json,sys,urllib.request
at,base,pid=sys.argv[1:4]
req=urllib.request.Request(base+f"/posts/{pid}", data=json.dumps({"content":"edited #shipit note"}).encode(),
  headers={"authorization":f"Bearer {at}","content-type":"application/json"}, method="PATCH")
d=json.load(urllib.request.urlopen(req))
assert d.get("content")=="edited #shipit note", d
assert d.get("edited_at") is not None, d
print("edit: ok")
PY

python3 - "$AT2" "$BASE" "$H1" <<'PY'
import json,sys,urllib.request
at,base,h1=sys.argv[1:4]
req=urllib.request.Request(base+"/dms", data=json.dumps({"handle":h1,"content":"secret hello"}).encode(),
  headers={"authorization":f"Bearer {at}","content-type":"application/json"}, method="POST")
d=json.load(urllib.request.urlopen(req))
assert d.get("content")=="secret hello", d
assert d.get("conversation_id"), d
open("/tmp/cid","w").write(d["conversation_id"])
print("dm send: ok", d["conversation_id"][:12])
PY
CID=$(cat /tmp/cid)

python3 - "$AT1" "$BASE" "$H2" <<'PY'
import json,sys,urllib.request
at,base,h2=sys.argv[1:4]
req=urllib.request.Request(base+"/dms", headers={"authorization":f"Bearer {at}"})
d=json.load(urllib.request.urlopen(req))
assert len(d["conversations"])>=1, d
assert d["conversations"][0]["peer_handle"]==h2, d
print("dm list: ok")
PY

python3 - "$AT1" "$BASE" "$CID" <<'PY'
import json,sys,urllib.request
at,base,cid=sys.argv[1:4]
req=urllib.request.Request(base+f"/dms/{cid}", headers={"authorization":f"Bearer {at}"})
d=json.load(urllib.request.urlopen(req))
assert any(m["content"]=="secret hello" for m in d["messages"]), d
print("dm thread: ok", len(d["messages"]))
PY

python3 - "$AT1" "$BASE" "$H2" <<'PY'
import json,sys,urllib.request
at,base,h2=sys.argv[1:4]
# reason required
try:
  req=urllib.request.Request(base+f"/users/{h2}/block", data=json.dumps({"reason":""}).encode(),
    headers={"authorization":f"Bearer {at}","content-type":"application/json"}, method="POST")
  urllib.request.urlopen(req)
  raise SystemExit("expected 400 for empty reason")
except urllib.error.HTTPError as e:
  assert e.code==400, e
req=urllib.request.Request(base+f"/users/{h2}/block", data=json.dumps({"reason":"smoke test"}).encode(),
  headers={"authorization":f"Bearer {at}","content-type":"application/json"}, method="POST")
urllib.request.urlopen(req)
try:
  req=urllib.request.Request(base+f"/users/{h2}", headers={"authorization":f"Bearer {at}"})
  urllib.request.urlopen(req)
  raise SystemExit("expected 404 after block")
except urllib.error.HTTPError as e:
  assert e.code==404, e
  print("block: ok", e.code)
PY

python3 - "$AT1" "$BASE" <<'PY'
import json,sys,urllib.request
at,base=sys.argv[1:3]
req=urllib.request.Request(base+"/me/bio", data=json.dumps({"bio":"building conr"}).encode(),
  headers={"authorization":f"Bearer {at}","content-type":"application/json"}, method="PATCH")
d=json.load(urllib.request.urlopen(req))
assert d.get("bio")=="building conr", d
print("bio: ok")
PY

echo ALL_OK
