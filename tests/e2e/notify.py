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
        if cmd == "PLAY":
            q = line.split(" ", 1)[1] if " " in line else ""; base = 300 if q == "muse" else 100
            player["queue"] = [{"id": base + i, "artist": "B", "title": f"P{i} {q}", "duration": 100} for i in range(1, 6)]; player["index"] = 1
        c.sendall(("OK " + json.dumps(player) if cmd == "STATE" else "OK sent").encode() + b"\n"); c.close()

def user(uid): return {"id": uid, "is_bot": False, "first_name": "Друг" if uid == FRIEND else "Я", "username": "friend" if uid == FRIEND else None}
def msg(uid, text): return {"message": {"message_id": 1, "date": 0, "chat": {"id": uid, "type": "private"}, "from": user(uid), "text": text}}
def cb(uid, data, mid): return {"callback_query": {"id": "q-" + data, "from": user(uid), "chat_instance": "1", "data": data,
                                                   "message": {"message_id": mid, "date": 0, "chat": {"id": uid, "type": "private"}}}}
script = [
    [msg(FRIEND, "/start")],
    [cb(OWNER, "a:y:99", "REQ")],
    [msg(FRIEND, "rammstein")],
    [], [], [],
    [msg(FRIEND, "/notify off")],
    [], [], [], [], [], [], [], [], [], [], [], [], [], [],
    [msg(FRIEND, "muse")],              # muted, but searched himself
    [], [],
]
st = {"n": 0, "id": 1, "REQ": None, "USERS": None, "UPD": None}

class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def reply(self, obj, code=200, raw=None):
        out = raw if raw is not None else json.dumps(obj).encode()
        self.send_response(code); self.send_header("Content-Length", str(len(out))); self.end_headers(); self.wfile.write(out)
    def do_GET(self):
        calls.append(("GET", self.path, self.headers.get("User-Agent")))
        if self.path == "/repos/genaRijoff/soundiya/releases/latest":
            return self.reply({"tag_name": "v0.1.11", "html_url": "https://github.com/genaRijoff/soundiya/releases/tag/v0.1.11"})
        if self.path == "/genaRijoff/soundiya/v0.1.11/src/control.rs":
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
def voice():
    time.sleep(13)
    player["queue"] = [{"id": 200 + i, "artist": "C", "title": f"V{i}", "duration": 100} for i in range(1, 6)]; player["index"] = 1
threading.Thread(target=voice, daemon=True).start()
p = subprocess.Popen([BIN, "run", "--config", cfg, "--socket", SOCK], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
time.sleep(26); p.terminate(); out, err = p.communicate(timeout=10)

fails = []
def check(c, w): print(("ok   " if c else "FAIL ") + w); c or fails.append(w)
post = [c for c in calls if c[0] != "GET"]
cards = [(b["chat_id"], b["disable_notification"], b["text"]) for m, b in post if m == "sendMessage" and "c:toggle" in json.dumps(b.get("reply_markup", {}))]
friend_play = [c for c in cards if "P1" in c[2]]
check((FRIEND, False) in [(u, s) for u, s, _ in friend_play], "friend's search: card with sound to the friend")
check((OWNER, True) in [(u, s) for u, s, _ in friend_play], "friend's search: silent card to the owner")
check((OWNER, False) not in [(u, s) for u, s, _ in friend_play], "owner not rung for the friend's music")
voice_cards = [c for c in cards if "V1" in c[2]]
check((OWNER, False) in [(u, s) for u, s, _ in voice_cards], "voice queue: owner gets the card with sound")
check(all(u != FRIEND for u, _, _ in voice_cards), "voice queue: muted friend gets nothing")
check(99 in json.load(open(os.path.join(WORK, "access.json")))["muted"], "/notify off saved")
check((FRIEND, True) in [(u, s) for u, s, t in cards if "muse" in t], "muted, his own search still brings the card, silently")
check((FRIEND, False) not in [(u, s) for u, s, t in cards if "muse" in t], "...and does not ring")
print("--- log"); print(err.strip())
print("all passed" if not fails else f"{len(fails)} failed"); sys.exit(1 if fails else 0)
