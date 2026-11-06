#!/usr/bin/env python3
"""soundiya-tg 0.1.9: the owner's /admin menu, end to end.

The owner opens /admin and, from the buttons alone: opens the listener list and a listener,
allows them voice messages, mutes their notifications, switches the voice mode, raises the
voice volume, and removes another listener (with a confirmation). A listener pressing the same
buttons (forged callbacks) changes nothing. Everything lands in access.json.
"""
import json, os, socket, subprocess, sys, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

BIN, WORK = sys.argv[1], sys.argv[2]
TOKEN = "123456:AAbbCCddEEffGGhhIIjjKKllMMnnOOpp_-"
OWNER, KEEP, DROP = 42, 88, 77
SOCK = os.path.join(WORK, "s.sock")
ACCESS = os.path.join(WORK, "access.json")
for f in ("access.json", "updates.json"):
    try: os.remove(os.path.join(WORK, f))
    except FileNotFoundError: pass
json.dump({"listeners": [{"id": DROP, "name": "Drop"}, {"id": KEEP, "name": "Keep"}], "muted": []}, open(ACCESS, "w"))
os.chmod(ACCESS, 0o600)

def soundiya():
    if os.path.exists(SOCK): os.remove(SOCK)
    srv = socket.socket(socket.AF_UNIX); srv.bind(SOCK); srv.listen(8)
    state = {"state": "idle", "index": 0, "position": 0, "repeat": False, "sleep": None, "signed_in": True, "volume": 50, "queue": []}
    while True:
        c, _ = srv.accept(); c.recv(4096); c.sendall(("OK " + json.dumps(state) + "\n").encode()); c.close()

calls, lock = [], threading.Lock()
def user(uid): return {"id": uid, "is_bot": False, "first_name": "U%d" % uid}
def msg(uid, text): return {"message": {"message_id": 1, "date": 0, "chat": {"id": uid, "type": "private"}, "from": user(uid), "text": text}}
def cb(uid, data): return {"callback_query": {"id": "q-%d-%s" % (uid, data), "from": user(uid), "chat_instance": "1", "data": data,
                                              "message": {"message_id": "MENU", "date": 0, "chat": {"id": uid, "type": "private"}}}}
script = [
    [msg(OWNER, "/admin")], [],
    [cb(OWNER, "x:list")],
    [cb(OWNER, f"x:l:{KEEP}")],
    [cb(OWNER, f"x:v:{KEEP}")],
    [cb(OWNER, f"x:n:{KEEP}")],
    [cb(KEEP, f"x:v:{KEEP}")],          # a listener's forged press
    [cb(KEEP, "x:vm")],
    [cb(OWNER, "x:home")],
    [cb(OWNER, "x:vm")],                # owners -> all
    [cb(OWNER, "x:vv:+")],
    [cb(OWNER, f"x:v:{KEEP}")],         # under «всем»: off for him, mode -> owners
    [cb(OWNER, "x:vv:x")],              # not a step: ignored
    [cb(OWNER, f"x:d:{DROP}")],
    [cb(OWNER, f"x:D:{DROP}")],
    [cb(OWNER, f"x:l:{DROP}")],         # gone meanwhile
    [cb(OWNER, "x:l:abc")],
]
st = {"n": 0, "id": 1, "MENU": 0}

class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_POST(self):
        m = self.path.rsplit("/", 1)[1]; body = json.loads(self.rfile.read(int(self.headers["Content-Length"])) or b"{}")
        with lock: calls.append((m, body))
        res = True
        if m == "getMe": res = {"id": 1, "is_bot": True, "first_name": "b", "username": "b_bot"}
        elif m == "getUpdates":
            time.sleep(0.4)
            with lock: n = st["n"]; st["n"] += 1
            res = []
            if n < len(script):
                for i, u in enumerate(json.loads(json.dumps(script[n]))):
                    mm = u.get("callback_query", {}).get("message")
                    if mm: mm["message_id"] = st["MENU"]
                    u["update_id"] = n * 10 + i + 1; res.append(u)
            else: time.sleep(1)
        elif m == "sendMessage":
            with lock:
                mid = st["id"]; st["id"] += 1
                if "x:list" in json.dumps(body.get("reply_markup", {})): st["MENU"] = mid
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
time.sleep(12); p.terminate()
try:
    out, err = p.communicate(timeout=10)
    stopped = True
except subprocess.TimeoutExpired:
    p.kill(); out, err = p.communicate(); stopped = False

fails = []
def check(c, w): print(("ok   " if c else "FAIL ") + w); c or fails.append(w)
edits = [b for m, b in calls if m == "editMessageText"]
toasts = [b.get("text", "") for m, b in calls if m == "answerCallbackQuery"]
access = json.load(open(ACCESS))
keep = next((l for l in access["listeners"] if l["id"] == KEEP), {})
check(stopped, "the bot stops on SIGTERM (no deadlock)")
check(st["MENU"] > 0, "/admin sends the menu")
check(any("Слушатели" in e["text"] and f"x:l:{KEEP}" in json.dumps(e["reply_markup"]) for e in edits), "the list links to each listener")
check(any("<b>Keep</b>" in e["text"] and f"x:v:{KEEP}" in json.dumps(e["reply_markup"]) for e in edits), "a listener's page has its switches")
check(any("Голосовые разрешены" in t for t in toasts), "voice messages allowed for the listener")
check(keep.get("voice") is not True and access.get("voice_from") == "owners"
      and any("запрещены · режим: отмеченным" in t for t in toasts), "under «всем» the button turns him off and the mode to «отмеченным»")
check(KEEP in access["muted"], "the listener's notifications muted")
check(any("Голосовые: все слушатели" in t for t in toasts), "voice mode switched (owners -> all)")
check(access.get("voice_volume") == 110, "voice volume +10")
check(all(l["id"] != DROP for l in access["listeners"]), "the other listener removed after confirming")
check(any("Забрать доступ у <b>Drop</b>" in e["text"] for e in edits), "removal asks first")
check(sum(1 for t in toasts if "Только владелец" in t) == 2, "a listener's presses are refused")
check(any("уже нет" in t for t in toasts), "a gone or bad listener id falls back to the list")
check(any(b.get("chat_id") == DROP and "закрыт" in b.get("text", "") for m, b in calls if m == "sendMessage"), "the removed listener is told")
check(any(m == "setMyCommands" and "admin" in json.dumps(b) for m, b in calls), "the command menu offers /admin")
print("--- log"); print(err.strip())
print("all passed" if not fails else f"{len(fails)} failed"); sys.exit(1 if fails else 0)
