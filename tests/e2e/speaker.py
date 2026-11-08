#!/usr/bin/env python3
"""«Алиса, передай …», end to end.

Off by default: the bot does not ask soundiya for messages, so Alice keeps such phrases. The
owner turns it on in /admin; then "Маше, что я буду через час" reaches the listener (and a
quiet copy the owner), Alice confirms; a second message within 10 s is refused; an unknown
name is refused; the owner is found by the Telegram first name.
"""
import json, os, socket, subprocess, sys, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

BIN, WORK = sys.argv[1], sys.argv[2]
TOKEN = "123456:AAbbCCddEEffGGhhIIjjKKllMMnnOOpp_-"
OWNER, FRIEND = 42, 77
SOCK = os.path.join(WORK, "s.sock")
for f in ("access.json", "updates.json"):
    try: os.remove(os.path.join(WORK, f))
    except FileNotFoundError: pass
json.dump({"listeners": [{"id": FRIEND, "name": "Маша🌸"}], "muted": []}, open(os.path.join(WORK, "access.json"), "w"))
os.chmod(os.path.join(WORK, "access.json"), 0o600)

said, polls, lock = [], [], threading.Lock()
st = {"n": 0, "id": 1, "MENU": 0, "enabled_at": None}
batches = [
    (0, ["Маше, что я буду через час", "Олегу привет"]),   # the second within 10 s: refused
    (11, ["Ёжику привет"]),                                 # nobody by that name
    (12.5, ["Олегу привет"]),                                # the owner, by first name
]
player = {"state": "idle", "index": 0, "position": 0, "repeat": False, "sleep": None, "signed_in": True, "volume": 50, "queue": []}

def soundiya():
    if os.path.exists(SOCK): os.remove(SOCK)
    srv = socket.socket(socket.AF_UNIX); srv.bind(SOCK); srv.listen(8)
    while True:
        c, _ = srv.accept(); line = c.recv(4096).decode().strip(); cmd, _, arg = line.partition(" ")
        if cmd == "MESSAGES":
            now = time.time()
            with lock:
                polls.append(now)
                first = polls[0]
                out = []
                while batches and now - first >= batches[0][0]:
                    out += batches.pop(0)[1]
            reply = "OK " + json.dumps(out, ensure_ascii=False)
        elif cmd == "SAY":
            said.append(arg); reply = "OK say"
        else:
            reply = "OK " + json.dumps(player)
        c.sendall(reply.encode() + b"\n"); c.close()

def user(uid): return {"id": uid, "is_bot": False, "first_name": "Олег" if uid == OWNER else "Маша"}
def msg(uid, text): return {"message": {"message_id": 1, "date": 0, "chat": {"id": uid, "type": "private"}, "from": user(uid), "text": text}}
def cb(uid, data): return {"callback_query": {"id": "q-" + data, "from": user(uid), "chat_instance": "1", "data": data,
                                              "message": {"message_id": "MENU", "date": 0, "chat": {"id": uid, "type": "private"}}}}
script = [[], [], [], [], [], [msg(OWNER, "/admin")], [], [cb(FRIEND, "x:sm")], [cb(OWNER, "x:sm")]]
calls = []

class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_POST(self):
        m = self.path.rsplit("/", 1)[1]; body = json.loads(self.rfile.read(int(self.headers["Content-Length"])) or b"{}")
        with lock: calls.append((time.time(), m, body))
        res = True
        if m == "getMe": res = {"id": 1, "is_bot": True, "first_name": "b", "username": "b_bot"}
        elif m == "getChat": res = {"id": body["chat_id"], "type": "private", "first_name": "Олег"}
        elif m == "getUpdates":
            time.sleep(0.6)
            with lock: n = st["n"]; st["n"] += 1
            res = []
            if n < len(script):
                for i, u in enumerate(json.loads(json.dumps(script[n]))):
                    mm = u.get("callback_query", {}).get("message")
                    if mm: mm["message_id"] = st["MENU"]
                    if "callback_query" in u and u["callback_query"]["from"]["id"] == OWNER: st["enabled_at"] = time.time()
                    u["update_id"] = n * 10 + i + 1; res.append(u)
            else: time.sleep(1)
        elif m == "sendMessage":
            with lock:
                mid = st["id"]; st["id"] += 1
                if "x:sm" in json.dumps(body.get("reply_markup", {})): st["MENU"] = mid
            res = {"message_id": mid, "date": 0, "chat": {"id": body["chat_id"], "type": "private"}}
        out = json.dumps({"ok": True, "result": res}).encode()
        self.send_response(200); self.send_header("Content-Length", str(len(out))); self.end_headers(); self.wfile.write(out)

threading.Thread(target=soundiya, daemon=True).start()
http = ThreadingHTTPServer(("127.0.0.1", 0), H); threading.Thread(target=http.serve_forever, daemon=True).start()
base = f"http://127.0.0.1:{http.server_address[1]}"
cfg = os.path.join(WORK, "config.json")
json.dump({"token": TOKEN, "users": [OWNER], "api": base, "updates": False}, open(cfg, "w")); os.chmod(cfg, 0o600)
time.sleep(0.3)
p = subprocess.Popen([BIN, "run", "--config", cfg, "--socket", SOCK], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
time.sleep(24); p.terminate(); out, err = p.communicate(timeout=10)

fails = []
def check(c, w): print(("ok   " if c else "FAIL ") + w); c or fails.append(w)
sends = [(b["chat_id"], b.get("text", ""), b.get("disable_notification")) for _, m, b in calls if m == "sendMessage"]
toasts = [b.get("text", "") for _, m, b in calls if m == "answerCallbackQuery"]
check(st["enabled_at"] is not None and polls and min(polls) > st["enabled_at"], "off by default: no MESSAGES before the owner turns it on")
check(any("Только владелец" in t for t in toasts), "a listener cannot turn it on")
check(json.load(open(os.path.join(WORK, "access.json"))).get("speaker_messages") is True, "the switch is saved")
check((FRIEND, "🗣 <b>С колонки:</b> я буду через час", False) in sends, "the listener gets the words, with a sound")
check(any(c == OWNER and t == "🗣 С колонки → Маша🌸: я буду через час" and q for c, t, q in sends), "the owner gets a quiet copy")
check((OWNER, "🗣 <b>С колонки:</b> привет", False) in sends, "the owner is found by the Telegram first name")
check(said[:3] == ["Отправила в Телеграм. Получатель: Маша", "Подождите немного и повторите", "Не нашла, кому передать"], f"Alice answers each ({said[:3]})")
check(said[3:4] == ["Отправила в Телеграм. Получатель: Олег"], "and confirms the owner's")
check(sum(1 for c, t, _ in sends if "Ёжик" in t) == 0, "an unknown name sends nothing")
check(TOKEN not in err, "the token is not in the log")
print("--- log"); print(err.strip())
print("all passed" if not fails else f"{len(fails)} failed"); sys.exit(1 if fails else 0)
