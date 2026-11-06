#!/usr/bin/env python3
"""soundiya-tg 0.1.8: voice messages, end to end.

The owner's voice message is fetched from Telegram, decoded by GStreamer (a stand-in for
`maind --gst-launch` that checks the pipeline), leveled (a quiet tone comes out at the automatic
level times /voicevol) and handed to soundiya with VOICE; the bot waits until soundiya reports
the message is over. Too long, too big, a listener's message (owners only by default) and a
second one meanwhile are refused before anything is downloaded; after /voice all a listener's
message plays and the owner is told. No file stays behind.
"""
import json, math, os, socket, struct, subprocess, sys, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

BIN, WORK = sys.argv[1], sys.argv[2]
TOKEN = "123456:AAbbCCddEEffGGhhIIjjKKllMMnnOOpp_-"
OWNER, LISTENER = 42, 77
SOCK = os.path.join(WORK, "s.sock")
VOICE_DIR = "/tmp/soundiya-tg-voice"
OGG = b"OggS-fake-opus-" + bytes(range(200))
for f in ("access.json", "updates.json", "gst-args.txt"):
    try: os.remove(os.path.join(WORK, f))
    except FileNotFoundError: pass
json.dump({"listeners": [{"id": LISTENER, "name": "Friend"}], "muted": []}, open(os.path.join(WORK, "access.json"), "w"))
os.chmod(os.path.join(WORK, "access.json"), 0o600)

# A quiet 2 s tone (RMS ~1060) as the "decoded" voice.
TONE = os.path.join(WORK, "tone.pcm")
open(TONE, "wb").write(b"".join(struct.pack("<h", int(1500 * math.sin(n * 440 * 2 * math.pi / 16000))) for n in range(32000)))
# Stand-in for `maind --gst-launch ...`: checks the pipeline, then "decodes" to the tone.
gst = os.path.join(WORK, "fake-gst.sh")
open(gst, "w").write(f"""#!/bin/sh
echo "$LD_LIBRARY_PATH $*" >> {WORK}/gst-args.txt
src=""; dst=""
for a in "$@"; do case "$a" in location=*.oga) src=${{a#location=}};; location=*.pcm) dst=${{a#location=}};; esac; done
[ -f "$src" ] && [ -n "$dst" ] || exit 1
cp {TONE} "$dst"
""")
os.chmod(gst, 0o755)

calls, received, lock = [], [], threading.Lock()
player = {"state": "playing", "index": 1, "position": 3, "repeat": False, "sleep": None, "signed_in": True,
          "volume": 50, "voice": False, "queue": [{"id": 1, "artist": "A", "title": "T", "duration": 100}]}
voice_seen = {}

def soundiya():
    if os.path.exists(SOCK): os.remove(SOCK)
    srv = socket.socket(socket.AF_UNIX); srv.bind(SOCK); srv.listen(8)
    while True:
        c, _ = srv.accept(); line = c.recv(4096).decode().strip(); cmd, _, arg = line.partition(" ")
        if cmd == "VOICE":
            received.append(line)
            # As soundiya does: read the file, delete it, report `voice` while it plays.
            voice_seen["dir"] = os.path.dirname(arg)
            voice_seen["mode"] = oct(os.stat(os.path.dirname(arg)).st_mode & 0o777)
            voice_seen["size"] = os.path.getsize(arg)
            pcm = open(arg, "rb").read(); samples = struct.unpack(f"<{len(pcm)//2}h", pcm)
            voice_seen.setdefault("rms", []).append(math.sqrt(sum(x * x for x in samples) / len(samples)))
            voice_seen["others"] = sorted(os.listdir(os.path.dirname(arg)))
            os.remove(arg)
            player["voice"] = True
            threading.Timer(2.0, lambda: player.__setitem__("voice", False)).start()
            reply = "OK voice 2.0 s"
        elif cmd == "STATE":
            reply = "OK " + json.dumps(player)
        else:
            received.append(line); reply = "OK sent"
        c.sendall(reply.encode() + b"\n"); c.close()

def user(uid): return {"id": uid, "is_bot": False, "first_name": "X" if uid == OWNER else "Friend"}
def voice_msg(uid, mid, duration, size, fid):
    return {"message": {"message_id": mid, "date": 0, "chat": {"id": uid, "type": "private"}, "from": user(uid),
                        "voice": {"file_id": fid, "file_unique_id": fid, "duration": duration, "mime_type": "audio/ogg", "file_size": size}}}
def text(uid, mid, t):
    return {"message": {"message_id": mid, "date": 0, "chat": {"id": uid, "type": "private"}, "from": user(uid), "text": t}}
script = [
    [text(OWNER, 9, "/voicevol 150")],
    [voice_msg(OWNER, 10, 2, len(OGG), "good")],
    [voice_msg(OWNER, 11, 2, len(OGG), "busy")],           # while the first plays
    [voice_msg(OWNER, 12, 61, 1000, "long")],
    [voice_msg(OWNER, 13, 5, 3 << 20, "big")],
    [voice_msg(LISTENER, 14, 2, 1000, "friend")],
    [text(LISTENER, 15, "/voice all")],
    [text(OWNER, 16, "/voice all")],
    [], [],
    [voice_msg(LISTENER, 17, 2, len(OGG), "friend2")],
]
st = {"n": 0, "id": 100}

class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def reply(self, obj=None, code=200, raw=None):
        out = raw if raw is not None else json.dumps(obj).encode()
        self.send_response(code); self.send_header("Content-Length", str(len(out))); self.end_headers(); self.wfile.write(out)
    def do_GET(self):
        calls.append(("GET", self.path))
        if self.path == f"/file/bot{TOKEN}/voice/file_1.oga":
            return self.reply(raw=OGG)
        self.reply({}, 404)
    def do_POST(self):
        m = self.path.rsplit("/", 1)[1]; body = json.loads(self.rfile.read(int(self.headers["Content-Length"])) or b"{}")
        with lock: calls.append((m, body))
        res = True
        if m == "getMe": res = {"id": 1, "is_bot": True, "first_name": "b", "username": "b_bot"}
        elif m == "getFile": res = {"file_id": body["file_id"], "file_unique_id": "u", "file_size": len(OGG), "file_path": "voice/file_1.oga"}
        elif m == "getUpdates":
            time.sleep(0.6)
            with lock: n = st["n"]; st["n"] += 1
            res = []
            if n < len(script):
                for i, u in enumerate(script[n]):
                    u = json.loads(json.dumps(u)); u["update_id"] = n * 10 + i + 1; res.append(u)
            else: time.sleep(1)
        elif m == "sendMessage":
            with lock: mid = st["id"]; st["id"] += 1
            res = {"message_id": mid, "date": 0, "chat": {"id": body["chat_id"], "type": "private"}}
        self.reply({"ok": True, "result": res})

threading.Thread(target=soundiya, daemon=True).start()
http = ThreadingHTTPServer(("127.0.0.1", 0), H); threading.Thread(target=http.serve_forever, daemon=True).start()
base = f"http://127.0.0.1:{http.server_address[1]}"
cfg = os.path.join(WORK, "config.json")
json.dump({"token": TOKEN, "users": [OWNER], "api": base, "updates": False, "gst": gst}, open(cfg, "w")); os.chmod(cfg, 0o600)
time.sleep(0.3)
p = subprocess.Popen([BIN, "run", "--config", cfg, "--socket", SOCK], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
time.sleep(16); p.terminate(); out, err = p.communicate(timeout=10)

fails = []
def check(c, w): print(("ok   " if c else "FAIL ") + w); c or fails.append(w)
posts = [c for c in calls if c[0] != "GET"]
texts = [(b.get("chat_id"), b.get("text", "")) for m, b in posts if m in ("sendMessage", "editMessageText")]
get_files = [b["file_id"] for m, b in posts if m == "getFile"]
gst_args = open(os.path.join(WORK, "gst-args.txt")).read() if os.path.exists(os.path.join(WORK, "gst-args.txt")) else ""
check(get_files == ["good", "friend2"], "only accepted messages are downloaded")
check([l for l in received if l.startswith("VOICE")] and received[0].startswith(f"VOICE {VOICE_DIR}/v42-10.pcm"), "soundiya gets VOICE with the decoded file")
check(voice_seen.get("size") == 64000, "the leveled file keeps its length")
check(voice_seen.get("others") == ["v77-17.pcm"], "the OGG is deleted after decoding")
rms = voice_seen.get("rms", [])
check(len(rms) == 2 and abs(rms[0] - 13500) < 400, f"quiet speech is leveled, times /voicevol 150 (RMS {rms[:1]})")
check("voice: speech -30 dBFS -> -8 dBFS (gain x12.7, /voicevol 150 %)" in err, "the log shows the levels")
check(json.load(open(os.path.join(WORK, "access.json"))).get("voice_volume") == 150
      and json.load(open(os.path.join(WORK, "access.json"))).get("voice_from") == "all", "/voicevol and /voice are saved")
check(voice_seen.get("mode") == "0o700", "the voice folder is private")
check("/system/vendor/quasar/libs --gst-launch filesrc" in gst_args and "! opusdec !" in gst_args
      and "audio/x-raw,format=S16LE,rate=16000,channels=1" in gst_args, "GStreamer decodes Opus to 16 kHz mono")
check(any("Голосовое проиграно (2 с)" in t for _, t in texts), "the owner sees: played")
check(any("Ещё играет предыдущее" in t for _, t in texts), "a second message meanwhile waits its turn")
check(any("длиннее 1 минуты" in t for _, t in texts), "over a minute is refused")
check(any("больше 2 МБ" in t for _, t in texts), "over 2 MB is refused")
check(any(c == LISTENER and "не разрешены" in t for c, t in texts), "listeners may not by default")
check(sum(1 for c, t in texts if c == LISTENER and "Это может только владелец" in t) == 1, "only the owner may change /voice")
check(any(c == LISTENER and "Голосовое проиграно" in t for c, t in texts), "after /voice all a listener's message plays")
check(any(c == OWNER and "Friend включил(а) голосовое" in t for c, t in texts), "the owner is told who sent it")
check(os.path.isdir(VOICE_DIR) and os.listdir(VOICE_DIR) == [], "nothing stays in the voice folder")
check(TOKEN not in err, "the token is not in the log")
print("--- log"); print(err.strip())
print("all passed" if not fails else f"{len(fails)} failed"); sys.exit(1 if fails else 0)
