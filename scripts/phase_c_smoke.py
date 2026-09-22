#!/usr/bin/env python3
"""Smoke-test Phase C APIs against a freshly started conr-server."""

from __future__ import annotations

import json
import os
import subprocess
import time
import urllib.error
import urllib.parse
import urllib.request

PORT = os.environ.get("CONR_PORT", "18085")
BASE = f"http://127.0.0.1:{PORT}"
DATABASE_URL = os.environ.get(
    "DATABASE_URL", f"postgres://{os.environ.get('USER', 'postgres')}@localhost/conr"
)


def req(method: str, path: str, token: str | None = None, body: dict | None = None):
    data = None if body is None else json.dumps(body).encode()
    headers = {}
    if token:
        headers["authorization"] = f"Bearer {token}"
    if body is not None:
        headers["content-type"] = "application/json"
    r = urllib.request.Request(BASE + path, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(r) as resp:
            raw = resp.read().decode()
            return resp.status, (json.loads(raw) if raw else {})
    except urllib.error.HTTPError as e:
        raw = e.read().decode()
        try:
            payload = json.loads(raw) if raw else {"error": raw}
        except json.JSONDecodeError:
            payload = {"error": raw}
        return e.code, payload


def main() -> None:
    env = os.environ.copy()
    env["DATABASE_URL"] = DATABASE_URL
    env["CONR_PORT"] = PORT

    subprocess.run(["fuser", "-k", f"{PORT}/tcp"], check=False, capture_output=True)
    subprocess.run(["cargo", "build", "-q"], check=True)
    log = open("/tmp/conr-phasec.log", "w")
    proc = subprocess.Popen(
        ["cargo", "run", "-q", "--", "server", "run"],
        env=env,
        stdout=log,
        stderr=subprocess.STDOUT,
    )
    try:
        for _ in range(50):
            try:
                urllib.request.urlopen(BASE + "/health")
                break
            except Exception:
                time.sleep(0.2)
        else:
            raise RuntimeError("server failed to start; see /tmp/conr-phasec.log")

        stamp = str(int(time.time()))[-5:]
        h1, h2 = f"za{stamp}", f"zb{stamp}"

        code, r1 = req("POST", "/auth/register", body={"handle": h1})
        assert code == 200, r1
        code, r2 = req("POST", "/auth/register", body={"handle": h2})
        assert code == 200, r2
        at1, at2 = r1["access_token"], r2["access_token"]
        print("users:", h1, h2)

        code, p1 = req(
            "POST",
            "/posts",
            token=at1,
            body={"content": f"hello #shipit @{h2} ready"},
        )
        assert code == 200, p1
        pid = p1["id"]
        print("post:", pid)

        code, f = req("POST", f"/users/{h1}/follow", token=at2)
        assert code == 200, f
        assert f["followed_by_me"] is True
        assert f["followers"] >= 1
        print("follow: ok")

        code, ff = req("GET", "/feed?filter=following", token=at2)
        assert code == 200, ff
        assert any(p["id"] == pid for p in ff["posts"]), ff
        print("following feed: ok")

        code, s = req("GET", "/search?" + urllib.parse.urlencode({"q": "shipit"}), token=at1)
        assert code == 200, s
        assert len(s["posts"]) >= 1, s
        print("search fts: ok", len(s["posts"]), "tags", s.get("tags"))

        code, t = req(
            "GET",
            "/search?" + urllib.parse.urlencode({"q": "#shipit"}),
            token=at1,
        )
        assert code == 200, t
        assert len(t["posts"]) >= 1, t
        print("search tag: ok", len(t["posts"]))

        code, n = req("GET", "/notifications", token=at2)
        assert code == 200, n
        kinds = {x["kind"] for x in n["notifications"]}
        assert "mention" in kinds, n
        print("bob notifs: ok", sorted(kinds), "unread", n["unread"])

        code, e = req(
            "PATCH",
            f"/posts/{pid}",
            token=at1,
            body={"content": "edited #shipit note"},
        )
        assert code == 200, e
        assert e["content"] == "edited #shipit note", e
        assert e.get("edited_at") is not None, e
        print("edit: ok")

        code, d = req(
            "POST",
            "/dms",
            token=at2,
            body={"handle": h1, "content": "secret hello"},
        )
        assert code == 200, d
        assert d["content"] == "secret hello", d
        cid = d["conversation_id"]
        print("dm send: ok", cid[:12])

        code, dl = req("GET", "/dms", token=at1)
        assert code == 200, dl
        assert len(dl["conversations"]) >= 1, dl
        assert dl["conversations"][0]["peer_handle"] == h2, dl
        print("dm list: ok")

        code, th = req("GET", f"/dms/{cid}", token=at1)
        assert code == 200, th
        assert any(m["content"] == "secret hello" for m in th["messages"]), th
        print("dm thread: ok", len(th["messages"]))

        code, _ = req("POST", f"/users/{h2}/block", token=at1, body={"reason": ""})
        assert code == 400, _
        code, _ = req(
            "POST", f"/users/{h2}/block", token=at1, body={"reason": "smoke test"}
        )
        assert code == 200, _
        code, _ = req("GET", f"/users/{h2}", token=at1)
        assert code == 404, _
        print("block: ok", code)

        code, b = req("PATCH", "/me/bio", token=at1, body={"bio": "building conr"})
        assert code == 200, b
        assert b.get("bio") == "building conr", b
        print("bio: ok")

        print("ALL_OK")
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()


if __name__ == "__main__":
    main()
