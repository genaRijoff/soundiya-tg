#!/usr/bin/env python3
"""soundiya-tg 0.1.6: an update from the fork, end to end.

The owner asks /updates and presses "Install": the bot downloads the release files (following the
redirect to file storage, as on github.com) and runs the station's autoupdater with SU_SOURCE.
A stranger's press and a forged tag change nothing. The watched issue is read from issue_repo.
"""
import hashlib, json, os, socket, subprocess, sys, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

BIN, WORK = sys.argv[1], sys.argv[2]
TOKEN = "123456:AAbbCCddEEffGGhhIIjjKKllMMnnOOpp_-"
OWNER, STRANGER = 42, 99
TAG = "v0.1.11-bot"
SOCK = os.path.join(WORK, "s.sock")
AU_DIR = os.path.join(WORK, "autoupdate")
DOWNLOAD_DIR = "/tmp/soundiya-tg-update"
for f in ("access.json", "updates.json"):
    try: os.remove(os.path.join(WORK, f))
    except FileNotFoundError: pass
os.makedirs(AU_DIR, exist_ok=True)
for f in ("update.log", "files.txt"):
    try: os.remove(os.path.join(AU_DIR, f))
    except FileNotFoundError: pass

# The release: the same files as an upstream soundiya release.
files = {n: f"#!/bin/sh\n# {n}\n".encode() for n in ("install.sh", "run.sh", "start.sh", "stop.sh", "S99soundiya")}
files["soundiya"] = b"BUILD-" + TAG.encode()
files["SHA256SUMS"] = "".join(f"{hashlib.sha256(b).hexdigest()}  {n}\n" for n, b in files.items()).encode()

# A stand-in for /data/opt/soundiya-autoupdate/soundiya-autoupdate.sh: checks the files it is given.
autoupdate = os.path.join(AU_DIR, "soundiya-autoupdate.sh")
open(autoupdate, "w").write("""#!/bin/sh
here=$(dirname "$0")
[ "$1" = once ] && [ "$SU_AUTO_INSTALL" = 1 ] && [ -d "$SU_SOURCE" ] || exit 2
ls "$SU_SOURCE" > "$here/files.txt"
cd "$SU_SOURCE" && sha256sum -c SHA256SUMS >/dev/null 2>&1 || { echo "checksums failed" >> "$here/update.log"; exit 1; }
echo "installed $(cat soundiya)" >> "$here/update.log"
""")

calls, received, lock = [], [], threading.Lock()
player = {"state": "playing", "index": 1, "position": 3, "repeat": False, "sleep": None, "signed_in": True,
          "volume": 50, "queue": [{"id": i, "artist": "A", "title": f"T{i}", "duration": 100} for i in range(1, 4)]}

def soundiya():
    if os.path.exists(SOCK): os.remove(SOCK)
    srv = socket.socket(socket.AF_UNIX); srv.bind(SOCK); srv.listen(8)
    while True:
        c, _ = srv.accept(); line = c.recv(4096).decode().strip(); cmd = line.split(" ")[0]
        if cmd != "STATE": received.append(line)
        c.sendall(("OK " + json.dumps(player) if cmd == "STATE" else "OK sent").encode() + b"\n"); c.close()

def user(uid): return {"id": uid, "is_bot": False, "first_name": "X"}
def msg(uid, text): return {"message": {"message_id": 1, "date": 0, "chat": {"id": uid, "type": "private"}, "from": user(uid), "text": text}}
def cb(uid, data, mid): return {"callback_query": {"id": "q-" + data, "from": user(uid), "chat_instance": "1", "data": data,
                                                   "message": {"message_id": mid, "date": 0, "chat": {"id": uid, "type": "private"}}}}
script = [
    [msg(OWNER, "/updates")],
    [], [],
    [cb(STRANGER, f"u:i:{TAG}", "UPD")],
    [cb(OWNER, "u:i:../../etc", "UPD")],
    [],
    [cb(OWNER, f"u:i:{TAG}", "UPD")],
]
st = {"n": 0, "id": 1, "UPD": None}

class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def reply(self, obj, code=200, raw=None):
        out = raw if raw is not None else json.dumps(obj).encode()
        self.send_response(code); self.send_header("Content-Length", str(len(out))); self.end_headers(); self.wfile.write(out)
    def do_GET(self):
        calls.append(("GET", self.path))
        if self.path == "/repos/genaRijoff/soundiya/releases/latest":
            return self.reply({"tag_name": TAG, "html_url": f"https://github.com/genaRijoff/soundiya/releases/tag/{TAG}"})
        if self.path == f"/genaRijoff/soundiya/{TAG}/src/control.rs":
            return self.reply(None, raw=b'match x { "STATE" => 1 }')
        if self.path == "/repos/ShadowBlade83/soundiya/issues/5":
            return self.reply({"state": "open", "comments": 2, "html_url": "https://github.com/ShadowBlade83/soundiya/issues/5"})
        prefix = f"/genaRijoff/soundiya/releases/download/{TAG}/"
        if self.path.startswith(prefix) and self.path[len(prefix):] in files:
            self.send_response(302); self.send_header("Location", "/storage/" + self.path[len(prefix):])
            self.send_header("Content-Length", "0"); self.end_headers(); return
        if self.path.startswith("/storage/") and self.path[9:] in files:
            return self.reply(None, raw=files[self.path[9:]])
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
                    mm = u.get("callback_query", {}).get("message")
                    if mm and isinstance(mm["message_id"], str): mm["message_id"] = st[mm["message_id"]] or 0
                    u["update_id"] = n * 10 + i + 1; res.append(u)
            else: time.sleep(1)
        elif m == "sendMessage":
            with lock:
                mid = st["id"]; st["id"] += 1
                if "u:i:" in json.dumps(body.get("reply_markup", {})): st["UPD"] = mid
            res = {"message_id": mid, "date": 0, "chat": {"id": body["chat_id"], "type": "private"}}
        self.reply({"ok": True, "result": res})

threading.Thread(target=soundiya, daemon=True).start()
http = ThreadingHTTPServer(("127.0.0.1", 0), H); threading.Thread(target=http.serve_forever, daemon=True).start()
base = f"http://127.0.0.1:{http.server_address[1]}"
cfg = os.path.join(WORK, "config.json")
json.dump({"token": TOKEN, "users": [OWNER], "api": base, "github_api": base, "github_raw": base,
           "github_download": base, "autoupdate": autoupdate, "watch_issue": 5}, open(cfg, "w")); os.chmod(cfg, 0o600)
time.sleep(0.3)
p = subprocess.Popen([BIN, "run", "--config", cfg, "--socket", SOCK], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
time.sleep(22); p.terminate(); out, err = p.communicate(timeout=10)

fails = []
def check(c, w): print(("ok   " if c else "FAIL ") + w); c or fails.append(w)
gets = [c[1] for c in calls if c[0] == "GET"]
posts = [c for c in calls if c[0] != "GET"]
texts = [b.get("text", "") for m, b in posts if m in ("sendMessage", "editMessageText")]
log = open(os.path.join(AU_DIR, "update.log")).read() if os.path.exists(os.path.join(AU_DIR, "update.log")) else ""
listed = open(os.path.join(AU_DIR, "files.txt")).read().split() if os.path.exists(os.path.join(AU_DIR, "files.txt")) else []
check("/repos/genaRijoff/soundiya/releases/latest" in gets, "releases come from the fork")
check("/repos/ShadowBlade83/soundiya/issues/5" in gets, "the watched issue is read from issue_repo")
check(any(TAG in t and "STATE" in t for t in texts), "/updates offers the fork release with STATE")
check(log.count("installed BUILD-" + TAG) == 1, "installed exactly once, checksums verified by the autoupdater")
check(sorted(listed) == sorted(files), "the autoupdater got all 7 release files")
check(sum(1 for g in gets if g.startswith("/storage/")) == 7, "downloads followed the redirect to file storage")
check(not any("../" in g for g in gets), "a forged tag downloads nothing")
check("PAUSE" in received, "the music is paused before the install")
check(any("Готово" in t for t in texts), "the owner sees: done")
check(not os.path.exists(DOWNLOAD_DIR), "the download folder is removed")
print("--- log"); print(err.strip())
print("all passed" if not fails else f"{len(fails)} failed"); sys.exit(1 if fails else 0)
