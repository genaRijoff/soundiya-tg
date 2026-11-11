#!/usr/bin/env python3
"""soundiya-tg 0.1.16: the web panel, end to end.

Off by default: nothing listens. The owner turns it on in /admin and sees the address and the
PIN there. Without a session every API call is refused; a wrong PIN counts down, five in a row
lock the sign-in and the owners are told. Changes must come from the panel's own page (Origin,
JSON) and to a local name (Host); oversized requests are refused. Signed in, the page gets the
player state (covers only from SoundCloud's CDN), likes, playlists, charts, search results, and
its commands reach soundiya as PLAYIDS, SEEK, PAUSE, VOLUME, OPENPL; the wave mixes tracks like
the likes but not the likes themselves. A new PIN signs everyone out.
Pass --keep to leave the bot running for screenshots (prints the PIN).
"""
import http.client, json, os, socket, subprocess, sys, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

BIN, WORK = sys.argv[1], sys.argv[2]
KEEP = "--keep" in sys.argv
TOKEN = "123456:AAbbCCddEEffGGhhIIjjKKllMMnnOOpp_-"
OWNER = 42
PORT = 18765
SOCK = os.path.join(WORK, "s.sock")
for f in ("access.json", "updates.json", "panel.json"):
    try: os.remove(os.path.join(WORK, f))
    except FileNotFoundError: pass
json.dump({"listeners": [], "muted": []}, open(os.path.join(WORK, "access.json"), "w"))
os.chmod(os.path.join(WORK, "access.json"), 0o600)

ART = "https://i1.sndcdn.com/artworks-%d-large.jpg"
# Made-up artists and titles (no real catalogue in the tests or the screenshots).
NAMES = [("Lumen Drift", "Kestrel"), ("Night Ferry", "Awake at Four"), ("Paper Moons", "Quiet Harbour"),
         ("Hollow Pines", "Says Nothing"), ("Velvet Static", "Green Hours"), ("Low Orbit", "Shapes in Fog"),
         ("Glasshouse", "Baby Blue"), ("Neon Atlas", "Copper Line"), ("Coastline", "Small Kingdom"), ("Marlow", "Late Train"),
         ("Ashgrove", "Lantern"), ("Polar Club", "Persona"), ("Kite Season", "Blurred"), ("Aurel", "Remember")]
def track(i, art=True):
    a, t = NAMES[i % len(NAMES)]
    return {"id": 1000 + i, "artist": a, "title": t, "duration": 180 + i * 7,
            "art": ART % i if art else "", "url": "https://soundcloud.com/x/t%d" % i}
LIKES = [track(i) for i in range(30)]
player = {"state": "playing", "index": 2, "position": 61, "repeat": False, "sleep": None, "signed_in": True, "volume": 50,
          "voice": False, "queue": [track(0), dict(track(1), art="https://evil.example/x.jpg"), track(2)]}
received, lock = [], threading.Lock()

def reply_for(cmd, arg):
    if cmd == "STATE": return json.dumps(player)
    if cmd == "LIKES": return json.dumps(LIKES)
    if cmd == "PLAYLISTS": return json.dumps([{"id": 500 + i, "title": t, "count": 10 + i, "art": ART % (40 + i), "user": "me"}
                                              for i, t in enumerate(["Утро", "Дорога", "Вечер", "Работа"])])
    if cmd == "CHART": return json.dumps([track(i) for i in range(3, 15)])
    if cmd == "FIND": return json.dumps([dict(track(5), title="Rain &amp; Fog")] + [track(i) for i in range(6, 11)])
    if cmd == "SIMILAR":
        seed = int(arg)
        # Each seed: two liked tracks (must be skipped) and new ones.
        return json.dumps([track(0), track(1)] + [dict(track(0), id=seed * 10 + k) for k in range(8)])
    if cmd == "PAUSE": player["state"] = "paused"; return "sent"
    if cmd == "RESUME": player["state"] = "playing"; return "sent"
    if cmd == "VOLUME": player["volume"] = int(arg); return "volume %s%%" % arg
    return "sent"

def soundiya():
    if os.path.exists(SOCK): os.remove(SOCK)
    srv = socket.socket(socket.AF_UNIX); srv.bind(SOCK); srv.listen(16)
    while True:
        c, _ = srv.accept(); line = c.recv(65536).decode().strip(); cmd, _, arg = line.partition(" ")
        if cmd != "STATE":
            with lock: received.append(line)
        c.sendall(("OK " + reply_for(cmd, arg) + "\n").encode()); c.close()

calls = []
def user(uid): return {"id": uid, "is_bot": False, "first_name": "Owner"}
def msg(uid, text): return {"message": {"message_id": 1, "date": 0, "chat": {"id": uid, "type": "private"}, "from": user(uid), "text": text}}
def cb(uid, data): return {"callback_query": {"id": "q-" + data, "from": user(uid), "chat_instance": "1", "data": data,
                                              "message": {"message_id": "MENU", "date": 0, "chat": {"id": uid, "type": "private"}}}}
st = {"n": 0, "id": 1, "MENU": 0, "queue": []}

class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_POST(self):
        m = self.path.rsplit("/", 1)[1]; body = json.loads(self.rfile.read(int(self.headers["Content-Length"])) or b"{}")
        with lock: calls.append((time.time(), m, body))
        res = True
        if m == "getMe": res = {"id": 1, "is_bot": True, "first_name": "b", "username": "b_bot"}
        elif m == "getChat": res = {"id": body["chat_id"], "type": "private", "first_name": "Owner"}
        elif m == "getUpdates":
            time.sleep(0.4)
            with lock: q = st["queue"]; st["queue"] = []
            res = []
            for u in q:
                u = json.loads(json.dumps(u))
                mm = u.get("callback_query", {}).get("message")
                if mm: mm["message_id"] = st["MENU"]
                st["n"] += 1; u["update_id"] = st["n"]; res.append(u)
        elif m in ("sendMessage", "editMessageText"):
            with lock:
                mid = st["id"]; st["id"] += 1
                if "x:pn" in json.dumps(body.get("reply_markup", {})) and m == "sendMessage": st["MENU"] = mid
            res = {"message_id": mid, "date": 0, "chat": {"id": body["chat_id"], "type": "private"}}
        out = json.dumps({"ok": True, "result": res}).encode()
        self.send_response(200); self.send_header("Content-Length", str(len(out))); self.end_headers(); self.wfile.write(out)

def tg(update):
    with lock: st["queue"].append(update)

threading.Thread(target=soundiya, daemon=True).start()
srv = ThreadingHTTPServer(("127.0.0.1", 0), H); threading.Thread(target=srv.serve_forever, daemon=True).start()
cfg = os.path.join(WORK, "config.json")
json.dump({"token": TOKEN, "users": [OWNER], "api": f"http://127.0.0.1:{srv.server_address[1]}", "updates": False,
           "panel_port": PORT}, open(cfg, "w")); os.chmod(cfg, 0o600)
time.sleep(0.3)
p = subprocess.Popen([BIN, "run", "--config", cfg, "--socket", SOCK], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)

fails = []
def check(c, w): print(("ok   " if c else "FAIL ") + w); c or fails.append(w)

HOST = f"127.0.0.1:{PORT}"
def req(method, path, body=None, headers=None, raw=None):
    h = {"Host": HOST}
    if body is not None:
        h.update({"Origin": f"http://{HOST}", "Content-Type": "application/json"})
    h.update(headers or {})
    c = http.client.HTTPConnection("127.0.0.1", PORT, timeout=10)
    data = raw if raw is not None else (json.dumps(body).encode() if body is not None else None)
    c.putrequest(method, path, skip_host=True, skip_accept_encoding=True)
    for k, v in h.items():
        if v is not None: c.putheader(k, v)
    if data is not None: c.putheader("Content-Length", str(len(data)))
    try:
        c.endheaders(data)
        r = c.getresponse(); text = r.read().decode(errors="replace")
    except OSError as e:
        return 0, {}, str(e)
    finally:
        c.close()
    return r.status, dict((k.lower(), v) for k, v in r.getheaders()), text

def listening():
    try: socket.create_connection(("127.0.0.1", PORT), timeout=1).close(); return True
    except OSError: return False

def wait(cond, secs=8):
    end = time.time() + secs
    while time.time() < end:
        if cond(): return True
        time.sleep(0.2)
    return False

try:
    time.sleep(3.5)
    check(not listening(), "off by default: nothing listens")
    tg(msg(OWNER, "/admin"))
    check(wait(lambda: st["MENU"] != 0), "the owner opens /admin")
    tg(cb(OWNER, "x:pn"))
    check(wait(listening), "the owner turns the panel on: it listens")
    pin = json.load(open(os.path.join(WORK, "panel.json")))["pin"]
    check(oct(os.stat(os.path.join(WORK, "panel.json")).st_mode & 0o777) == "0o600", "panel.json is 0600")
    edits = [b for _, m, b in calls if m == "editMessageText" and "Веб-панель: включена" in b.get("text", "")]
    check(any(f"PIN <tg-spoiler>{pin}</tg-spoiler>" in b["text"] and f":{PORT}" in b["text"] for b in edits), "/admin shows the address and the PIN")
    check(json.load(open(os.path.join(WORK, "access.json"))).get("panel") is True, "the switch is saved")

    s, h, body = req("GET", "/")
    check(s == 200 and "soundiya" in body and "default-src 'none'" in h.get("content-security-policy", ""), "the page, with a strict CSP")
    check(req("GET", "/app.js")[0] == 200 and req("GET", "/app.css")[0] == 200, "script and styles")
    check(req("GET", "/api/state")[0] == 401, "no session: the state is refused")
    check(req("POST", "/api/cmd", {"do": "next"})[0] == 401, "no session: commands are refused")
    check(req("GET", "/api/me")[2] == '{"signed_in":false}', "/api/me says signed out")

    # Wrong PIN once, then the right one.
    wrong = "000000" if pin != "000000" else "111111"
    s, _, body = req("POST", "/api/login", {"pin": wrong})
    check(s == 401 and "Осталось попыток: 4" in body, "a wrong PIN counts down")
    s, h, _ = req("POST", "/api/login", {"pin": pin})
    cookie = h.get("set-cookie", "")
    check(s == 200 and "HttpOnly" in cookie and "SameSite=Strict" in cookie, "the right PIN: an HttpOnly, SameSite cookie")
    sid = cookie.split(";")[0]
    A = {"Cookie": sid}

    # Requests from elsewhere.
    check(req("POST", "/api/cmd", {"do": "next"}, {**A, "Origin": None})[0] == 403, "a change without Origin is refused")
    check(req("POST", "/api/cmd", {"do": "next"}, {**A, "Origin": "http://evil.example"})[0] == 403, "a change from another site is refused")
    check(req("POST", "/api/cmd", {"do": "next"}, {**A, "Content-Type": "text/plain"})[0] == 415, "a form-like body is refused")
    check(req("GET", "/api/state", None, {**A, "Host": "evil.example:%d" % PORT})[0] == 421, "another site's name (DNS rebinding) is refused")
    check(req("POST", "/api/cmd", None, A, raw=b"x" * 5000)[0] in (413, 403), "a big body is refused")
    big = req("GET", "/", None, {"X-Big": "y" * 9000})
    # Answered 431 and closed; the client may see the close before the answer.
    check(big[0] in (431, 0) and "soundiya" not in big[2], f"a huge head is refused ({big[0]})")
    check(req("PUT", "/api/state", None, A)[0] == 405, "other methods are refused")
    with lock: received.clear()

    # Signed in.
    s, _, body = req("GET", "/api/state", None, A); state = json.loads(body)
    check(s == 200 and state["queue"][0]["art"] == ART % 0 and state["queue"][1]["art"] == "", "state: covers only from sndcdn.com")
    s, _, body = req("POST", "/api/list", {"kind": "likes"}, A)
    check(s == 200 and len(json.loads(body)) == 30, "likes are listed")
    s, _, body = req("POST", "/api/list", {"kind": "playlists"}, A)
    check(s == 200 and json.loads(body)[0]["title"] == "Утро", "playlists are listed")
    s, _, body = req("POST", "/api/list", {"kind": "chart", "trending": True, "genre": "рок"}, A)
    check(s == 200 and len(json.loads(body)) == 12, "a chart is listed")
    s, _, body = req("POST", "/api/list", {"kind": "find", "q": "bonobo\nNEXT"}, A)
    check(s == 200 and len(json.loads(body)) == 6, "search results")
    check(json.loads(body)[0]["title"] == "Rain & Fog", "escaped titles are shown as typed")
    req("POST", "/api/cmd", {"do": "play", "ids": [1005, 1006, 1007]}, A)
    req("POST", "/api/cmd", {"do": "seek", "to": 90}, A)
    req("POST", "/api/cmd", {"do": "toggle"}, A)
    req("POST", "/api/cmd", {"do": "volume", "level": 40}, A)
    req("POST", "/api/cmd", {"do": "playlist", "id": 501}, A)
    req("POST", "/api/cmd", {"do": "sleep", "minutes": 30}, A)
    req("POST", "/api/cmd", {"do": "jump", "n": 3}, A)
    s, _, body = req("POST", "/api/cmd", {"do": "rm", "path": "/"}, A)
    check(s == 400, "an unknown command is refused")
    s, _, body = req("POST", "/api/cmd", {"do": "wave"}, A)
    with lock: got = list(received)
    print("   soundiya got:", got)
    check("FIND bonobo NEXT" in got, "a search stays one line")
    for want in ["PLAYIDS 1005,1006,1007", "SEEK 29", "PAUSE", "VOLUME 40", "OPENPL 501", "SLEEP 30", "JUMP 3"]:
        check(want in got, "→ " + want)
    wave = [l for l in got if l.startswith("PLAYIDS ") and l != "PLAYIDS 1005,1006,1007"]
    ids = [int(x) for x in wave[0].split(" ", 1)[1].split(",")] if wave else []
    liked = {t["id"] for t in LIKES}
    check(len(ids) == 25 and not (set(ids) & liked) and len(set(ids)) == 25, f"the wave: 25 new tracks, no likes ({len(ids)})")
    check(len({i // 10 for i in ids[:5]}) == 5, "the wave mixes its sources")

    # Logout, then the lock.
    s, h, _ = req("POST", "/api/logout", {}, A)
    check(req("GET", "/api/state", None, A)[0] == 401, "after logout the session is gone")
    s, h, _ = req("POST", "/api/login", {"pin": pin}); sid2 = h.get("set-cookie", "").split(";")[0]
    for _ in range(4): req("POST", "/api/login", {"pin": wrong})
    s, _, body = req("POST", "/api/login", {"pin": wrong})
    check(s == 429 and "Попробуйте через 15 мин" in body, "five wrong PINs in a row lock the sign-in")
    s, _, _ = req("POST", "/api/login", {"pin": pin})
    check(s == 429, "locked: even the right PIN waits")
    check(wait(lambda: any(m == "sendMessage" and "неверных PIN" in b.get("text", "") for _, m, b in calls)), "the owner is told")
    check(req("GET", "/api/state", None, {"Cookie": sid2})[0] == 200, "a browser signed in before keeps working")

    if KEEP:
        # A fresh PIN (and no lock) for the screenshots; playing, with a cover.
        player.update(state="playing", position=74)
        player["queue"][1]["art"] = ART % 1
        tg(cb(OWNER, "x:pin")); time.sleep(2)
        print("PIN", json.load(open(os.path.join(WORK, "panel.json")))["pin"], flush=True)
        input()
    else:
        tg(cb(OWNER, "x:pin"))
        check(wait(lambda: req("GET", "/api/state", None, {"Cookie": sid2})[0] == 401), "a new PIN signs everyone out")
        tg(cb(OWNER, "x:pn"))
        check(wait(lambda: not listening()), "turned off: it stops listening")
finally:
    p.terminate(); out, err = p.communicate(timeout=10)

check(TOKEN not in err and pin not in err, "neither the token nor the PIN is in the log")
check("panel: listening" in err and "panel: turned off" in err or KEEP, "the log tells on and off")
print("--- log"); print(err.strip())
print("all passed" if not fails else f"{len(fails)} failed"); sys.exit(1 if fails else 0)
