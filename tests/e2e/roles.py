#!/usr/bin/env python3
"""soundiya-tg 0.1.3: owner/listener/stranger roles and the GitHub update check, end to end."""
import json, os, socket, subprocess, sys, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

BIN, WORK = sys.argv[1], sys.argv[2]
TOKEN = "123456:AAbbCCddEEffGGhhIIjjKKllMMnnOOpp_-"
OWNER, FRIEND = 42, 99
SOCK = os.path.join(WORK, "s.sock")
for f in ("access.json", "updates.json"):
    try: os.remove(os.path.join(WORK, f))
    except FileNotFoundError: pass
calls, received, lock = [], [], threading.Lock()
player = {"state": "playing", "index": 1, "position": 3, "repeat": False, "sleep": None, "signed_in": True,
          "volume": 50, "queue": [{"id": i, "artist": "A", "title": f"T{i}", "duration": 100} for i in range(1, 6)]}

def soundiya():
    if os.path.exists(SOCK): os.remove(SOCK)
    srv = socket.socket(socket.AF_UNIX); srv.bind(SOCK); srv.listen(8)
    while True:
        c, _ = srv.accept(); line = c.recv(4096).decode().strip(); cmd = line.split(" ")[0]
        if cmd != "STATE": received.append(line)
        if cmd == "NEXT": player["index"] = min(player["index"] + 1, 5)
        c.sendall(("OK " + json.dumps(player) if cmd == "STATE" else "OK sent").encode() + b"\n"); c.close()

def user(uid): return {"id": uid, "is_bot": False, "first_name": 'Друг <b>"x"</b>' if uid == FRIEND else "Я", "username": "friend" if uid == FRIEND else None}
def msg(uid, text): return {"message": {"message_id": 1, "date": 0, "chat": {"id": uid, "type": "private"}, "from": user(uid), "text": text}}
def cb(uid, data, mid): return {"callback_query": {"id": "q-" + data, "from": user(uid), "chat_instance": "1", "data": data,
                                                   "message": {"message_id": mid, "date": 0, "chat": {"id": uid, "type": "private"}}}}
script = [
    [msg(FRIEND, "/start")],
    [cb(OWNER, "a:y:99", "REQ")],
    [msg(FRIEND, "/next")],
    [cb(FRIEND, "s:99999", 7), cb(FRIEND, "j:-1", 7), cb(FRIEND, "q:99999999999999999999999", 7), cb(FRIEND, "c:../../x", 7), cb(FRIEND, "zz", 7)],
    [msg(FRIEND, "x" * 4096 + "\n\u0000\u001b[31m")],
    [msg(FRIEND, "/updates")],
    [cb(FRIEND, "u:i:v0.1.11", 7)],
    [cb(FRIEND, "a:d:99", 7)],
    [msg(OWNER, "/users")],
    [msg(OWNER, "/updates")],
    [cb(OWNER, "u:s:v0.1.11", "UPD")],
    [cb(OWNER, "a:d:99", "USERS")],
    [msg(FRIEND, "/next")],
    [cb(OWNER, "a:n:99", "REQ")],
    [msg(FRIEND, "/start")],
    [msg(FRIEND, "/start")],
]
st = {"n": 0, "id": 1, "REQ": None, "USERS": None, "UPD": None}

class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def reply(self, obj, code=200, raw=None):
        out = raw if raw is not None else json.dumps(obj).encode()
        self.send_response(code); self.send_header("Content-Length", str(len(out))); self.end_headers(); self.wfile.write(out)
    def do_GET(self):
        calls.append(("GET", self.path, self.headers.get("User-Agent")))
        if self.path == "/repos/ShadowBlade83/soundiya/releases/latest":
            return self.reply({"tag_name": "v0.1.11", "html_url": "https://github.com/ShadowBlade83/soundiya/releases/tag/v0.1.11"})
        if self.path == "/ShadowBlade83/soundiya/v0.1.11/src/control.rs":
            return self.reply(None, raw=b'match x { "STATE" => 1 }')
        if self.path == "/repos/ShadowBlade83/soundiya/issues/5":
            return self.reply({"state": "open", "comments": 2, "html_url": "https://github.com/ShadowBlade83/soundiya/issues/5"})
        self.reply({}, 404)
    def do_POST(self):
        m = self.path.rsplit("/", 1)[1]; body = json.loads(self.rfile.read(int(self.headers["Content-Length"])) or b"{}")
        with lock: calls.append((m, body))
        res = True
        if m == "getMe": res = {"id": 1, "is_bot": True, "first_name": "b", "username": "b_bot"}
        elif m == "getUpdates":
            time.sleep(0.7)
            with lock: n = st["n"]; st["n"] += 1
            res = []
            if n < len(script):
                for i, u in enumerate(json.loads(json.dumps(script[n]))):
                    if "callback_query" in u:
                        mm = u["callback_query"]["message"]
                        if isinstance(mm["message_id"], str): mm["message_id"] = st[mm["message_id"]] or 0
                    u["update_id"] = n * 10 + i + 1; res.append(u)
            else: time.sleep(1)
        elif m == "sendMessage":
            with lock:
                mid = st["id"]; st["id"] += 1
                kb = json.dumps(body.get("reply_markup", {}))
                if "a:y:99" in kb: st["REQ"] = mid
                if "a:d:99" in kb or body["text"].startswith("👥"): st["USERS"] = mid
                if "u:s:" in kb: st["UPD"] = mid
            res = {"message_id": mid, "date": 0, "chat": {"id": body["chat_id"], "type": "private"}}
        self.reply({"ok": True, "result": res})

threading.Thread(target=soundiya, daemon=True).start()
http = ThreadingHTTPServer(("127.0.0.1", 0), H); threading.Thread(target=http.serve_forever, daemon=True).start()
base = f"http://127.0.0.1:{http.server_address[1]}"
cfg = os.path.join(WORK, "config.json")
json.dump({"token": TOKEN, "users": [OWNER], "api": base, "github_api": base, "github_raw": base, "watch_issue": 5}, open(cfg, "w")); os.chmod(cfg, 0o600)
time.sleep(0.3)
p = subprocess.Popen([BIN, "run", "--config", cfg, "--socket", SOCK], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
time.sleep(26); p.terminate(); out, err = p.communicate(timeout=10)

fails = []
def check(c, w): print(("ok   " if c else "FAIL ") + w); c or fails.append(w)
sends = [b for m, b in [c for c in calls if c[0] != "GET"] if m == "sendMessage"]
edits = [b for m, b in [c for c in calls if c[0] != "GET"] if m == "editMessageText"]
answers = [b.get("text", "") for m, b in [c for c in calls if c[0] != "GET"] if m == "answerCallbackQuery"]
to = lambda uid: [b["text"] for b in sends if b["chat_id"] == uid]
check(any("Запрос на доступ" in t for t in to(FRIEND)), "stranger told the request went to the owner")
check(any("Просит доступ" in t and "Друг &lt;b&gt;&quot;x&quot;&lt;/b&gt;" in t and "@friend" in t for t in to(OWNER)), "owner got the request, name HTML-escaped")
check(err.count("access request from user 99") == 2, "revoked user could ask again once; refused one is silenced")
check(p.returncode is not None and "panicked" not in err, "forged buttons and junk text did not crash the bot")
check(any(l.startswith("PLAY ") and len(l) <= 5 + 300 and "\x00" not in l and "\x1b" not in l for l in received), "junk text reached soundiya cleaned and bounded")
check(any("Доступ к пульту открыт" in t for t in to(FRIEND)), "friend welcomed after approval")
check(received.count("NEXT") == 1, "listener /next reached soundiya once (not after revoke)")
check(any("только владелец" in t for t in to(FRIEND)), "listener refused /updates")
check(answers.count("⛔ Только владелец") == 2, "listener refused install and access buttons")
check(any(t.startswith("👥") and "Друг &lt;b&gt;" in t for t in to(OWNER)), "/users lists the listener, escaped")
upd = [t for t in to(OWNER) if "v0.1.11" in t]
check(bool(upd) and "есть STATE" in upd[0] and "Issue #5: открыт, комментариев: 2" in upd[0], "/updates shows release with STATE and issue status")
check(json.load(open(os.path.join(WORK, "updates.json"))).get("skipped") == "v0.1.11", "skip remembered in updates.json")
acc = json.load(open(os.path.join(WORK, "access.json")))
check(acc["listeners"] == [], "listener removed from access.json")
check(not any(r.startswith(("SLEEP 99999", "JUMP -1")) for r in received), "forged values never reached soundiya unvalidated")
check(oct(os.stat(os.path.join(WORK, "access.json")).st_mode & 0o777) == "0o600", "access.json is private")
check(any("закрыт" in t for t in to(FRIEND)), "friend told access was closed")
check(all((c[2] or "").startswith("soundiya-tg/") for c in calls if c[0] == "GET"), "GitHub requests carry a User-Agent")
check(TOKEN.split(":")[1] not in err, "token never in the log")
print("--- log"); print(err.strip())
print("all passed" if not fails else f"{len(fails)} failed"); sys.exit(1 if fails else 0)
