#!/usr/bin/env python3
"""End-to-end check of soundiya-tg: a fake Bot API on 127.0.0.1 and a fake soundiya socket.

Usage: sim.py BINARY WORKDIR
"""
import json
import os
import socket
import subprocess
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

BINARY, WORK = sys.argv[1], sys.argv[2]
TOKEN = "123456:AAbbCCddEEffGGhhIIjjKKllMMnnOOpp_-"
USER, INTRUDER = 42, 99
SOCK = os.path.join(WORK, "soundiya.control")
calls = []
lock = threading.Lock()

# ---------------- fake soundiya ----------------
player = {
    "state": "playing", "index": 1, "position": 10, "repeat": False, "sleep": None,
    "signed_in": True, "volume": 50,
    "queue": [{"id": i, "artist": f"Artist {i}", "title": f"Track <{i}>", "duration": 180} for i in range(1, 13)],
}
received = []


def soundiya_server():
    if os.path.exists(SOCK):
        os.remove(SOCK)
    srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    srv.bind(SOCK)
    srv.listen(8)
    while True:
        conn, _ = srv.accept()
        data = b""
        while not data.endswith(b"\n"):
            chunk = conn.recv(4096)
            if not chunk:
                break
            data += chunk
        line = data.decode().strip()
        cmd, _, arg = line.partition(" ")
        received.append(line)
        with lock:
            reply = "OK sent"
            if cmd == "STATE":
                player["position"] += 1
                reply = "OK " + json.dumps(player, ensure_ascii=False)
            elif cmd == "NEXT":
                player["index"] = min(player["index"] + 1, len(player["queue"]))
            elif cmd == "PREV":
                player["index"] = max(player["index"] - 1, 1)
            elif cmd == "JUMP":
                player["index"] = int(arg)
            elif cmd == "PAUSE":
                player["state"] = "paused"
            elif cmd == "RESUME":
                player["state"] = "playing"
            elif cmd == "SLEEP":
                player["sleep"] = None if arg == "off" else int(arg)
            elif cmd == "VOLUME":
                v = player["volume"]
                v = min(100, v + int(arg[1:])) if arg.startswith("+") else max(0, v - int(arg[1:])) if arg.startswith("-") else int(arg)
                player["volume"] = v
                reply = f"OK volume {v}%"
            elif cmd not in ("PLAY", "FAV", "SEEK", "REPEAT"):
                reply = "ERROR unknown command"
        conn.sendall((reply + "\n").encode())
        conn.close()


# ---------------- fake Bot API ----------------
def msg(user, text, chat_type="private"):
    return {"message": {"message_id": 1000 + len(text), "date": 0, "chat": {"id": user, "type": chat_type},
                        "from": {"id": user, "is_bot": False, "first_name": "x"}, "text": text}}


def cb(user, data, message_id):
    return {"callback_query": {"id": f"cb-{data}", "from": {"id": user, "is_bot": False, "first_name": "x"},
                               "chat_instance": "1", "data": data,
                               "message": {"message_id": message_id, "date": 0, "chat": {"id": user, "type": "private"}}}}


script = [
    [msg(USER, "/start")],
    [msg(INTRUDER, "/next"), cb(INTRUDER, "c:next", 1)],
    [msg(USER, "/next")],
    [msg(USER, "/queue")],
    [cb(USER, "j:7", "QUEUE")],
    [cb(USER, "c:toggle", "CARD")],
    [cb(USER, "m:sleep", "CARD")],
    [cb(USER, "s:30", "CARD")],
    [msg(USER, "/vol 40")],
    [cb(USER, "c:volup", "CARD")],
    [cb(USER, "c:voldown", "CARD")],
    [cb(USER, "c:voldown", "CARD")],
    [msg(USER, "daft punk\none more time")],
    [msg(USER, "/next", chat_type="group")],
]
step = {"n": 0, "next_id": 1, "last_card": None, "last_queue": None}


class Api(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_POST(self):
        prefix = f"/bot{TOKEN}/"
        if not self.path.startswith(prefix):
            self.send_response(404)
            self.end_headers()
            self.wfile.write(b'{"ok":false,"error_code":404,"description":"Not Found"}')
            return
        method = self.path[len(prefix):]
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])) or b"{}")
        with lock:
            calls.append((method, body))
        result = True
        if method == "getMe":
            result = {"id": 1, "is_bot": True, "first_name": "t", "username": "test_soundiya_bot"}
        elif method == "getUpdates":
            time.sleep(0.8)
            with lock:
                n = step["n"]
                step["n"] += 1
            if n < len(script):
                updates = []
                for i, u in enumerate(script[n]):
                    u = json.loads(json.dumps(u))
                    if "callback_query" in u:
                        m = u["callback_query"]["message"]
                        if m["message_id"] == "QUEUE":
                            m["message_id"] = step["last_queue"]
                        elif m["message_id"] == "CARD":
                            m["message_id"] = step["last_card"]
                    u["update_id"] = n * 10 + i + 1
                    updates.append(u)
                result = updates
            else:
                time.sleep(1)
                result = []
        elif method == "sendMessage":
            with lock:
                mid = step["next_id"]
                step["next_id"] += 1
                if "reply_markup" in body and any(b.get("callback_data") == "c:toggle" for r in body["reply_markup"]["inline_keyboard"] for b in r):
                    step["last_card"] = mid
                if body["text"].startswith("📃"):
                    step["last_queue"] = mid
            result = {"message_id": mid, "date": 0, "chat": {"id": body["chat_id"], "type": "private"}, "text": body["text"]}
        out = json.dumps({"ok": True, "result": result}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)


def main():
    threading.Thread(target=soundiya_server, daemon=True).start()
    http = ThreadingHTTPServer(("127.0.0.1", 0), Api)
    threading.Thread(target=http.serve_forever, daemon=True).start()
    port = http.server_address[1]
    config = os.path.join(WORK, "config.json")
    with open(config, "w") as f:
        json.dump({"token": TOKEN, "users": [USER], "api": f"http://127.0.0.1:{port}"}, f)
    os.chmod(config, 0o600)
    time.sleep(0.3)
    proc = subprocess.Popen([BINARY, "run", "--config", config, "--socket", SOCK],
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    time.sleep(16)
    proc.terminate()
    out, err = proc.communicate(timeout=10)

    fails = []

    def check(cond, what):
        print(("ok   " if cond else "FAIL ") + what)
        if not cond:
            fails.append(what)

    sends = [b for m, b in calls if m == "sendMessage"]
    edits = [b for m, b in calls if m == "editMessageText"]
    answers = [b for m, b in calls if m == "answerCallbackQuery"]
    check(any(m == "setMyCommands" for m, _ in calls), "command menu set")
    to_intruder = [b for b in sends + edits if b["chat_id"] != USER]
    check(len(to_intruder) == 1 and "Запрос на доступ" in to_intruder[0]["text"], "stranger got only the access-request notice")
    check(any("a:y:99" in json.dumps(b.get("reply_markup", {})) for b in sends if b["chat_id"] == USER), "owner got the request with buttons")
    check(any("Пульт" in b["text"] or "пульт" in b["text"] for b in sends), "/start answers with help")
    check(any(b.get("text", "").startswith("🔒 Нет доступа") for b in answers), "intruder button refused")
    check("NEXT" in received and received.count("NEXT") == 1, "only the allowed /next reached soundiya")
    check("JUMP 7" in received, "queue button jumps to track 7")
    check(any(b["text"].startswith("📃") and "▶️ <b>7." in b["text"] for b in edits), "queue redrawn with track 7 marked")
    check("PAUSE" in received, "toggle pauses a playing track")
    check(any("s:30" in json.dumps(b.get("reply_markup", {})) for b in edits), "sleep menu shown on the card")
    check("SLEEP 30" in received, "sleep 30 sent")
    check(any("😴 через 30 мин" in b["text"] for b in edits + sends), "card shows the sleep timer")
    check(any("volume 40%" in b["text"] for b in sends), "/vol answers with the volume")
    check("VOLUME +10" in received and received.count("VOLUME -10") == 2, "volume buttons send +10 / -10")
    check(any(b.get("text") == "🔊 50%" for b in answers) and any(b.get("text") == "🔊 30%" for b in answers), "volume toast shows the level")
    check(any("🔊 30%" in b["text"] for b in edits), "card shows the new volume")
    check("PLAY daft punk one more time" in received, "plain text searches, on one line")
    check(any(m == "deleteMessage" for m, _ in calls), "old card deleted when a new one is posted")
    check(any("Track &lt;" in b["text"] for b in sends + edits), "titles HTML-escaped")
    check(TOKEN not in out + err and TOKEN.split(":")[1] not in out + err, "token never in the bot's output")
    check("access request from user 99" in err, "stranger logged")
    check("ignored a message from user 42 in a group chat" in err, "group message ignored")
    print("--- bot stderr ---")
    print(err.strip())
    print("--- soundiya received ---")
    print(received[-0:].__repr__()[:0] or ", ".join(l for l in received if l != "STATE"))
    print("all passed" if not fails else f"{len(fails)} failed")
    return 1 if fails else 0


sys.exit(main())
