#!/usr/bin/env python3
"""soundiya-tg recovers from Telegram outages: down at start, and down mid-run."""
import json, os, socket, subprocess, sys, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

BIN, WORK = sys.argv[1], sys.argv[2]
TOKEN = "123456:AAbbCCddEEffGGhhIIjjKKllMMnnOOpp_-"
PORT = 18765
calls = []
pending = []


class Api(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_POST(self):
        method = self.path.rsplit("/", 1)[1]
        self.rfile.read(int(self.headers["Content-Length"]))
        calls.append((time.time(), method))
        result = True
        if method == "getMe":
            result = {"id": 1, "is_bot": True, "first_name": "t", "username": "t_bot"}
        elif method == "getUpdates":
            if pending:
                result = [pending.pop(0)]
            else:
                time.sleep(1); result = []
        elif method == "sendMessage":
            result = {"message_id": 1, "date": 0, "chat": {"id": 42, "type": "private"}}
        out = json.dumps({"ok": True, "result": result}).encode()
        self.send_response(200); self.send_header("Content-Length", str(len(out))); self.end_headers(); self.wfile.write(out)


def serve():
    srv = ThreadingHTTPServer(("127.0.0.1", PORT), Api)
    srv.allow_reuse_address = True
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv


def msg(uid, text):
    return {"update_id": int(time.time()*1000) % 10**9, "message": {"message_id": 5, "date": 0,
            "chat": {"id": uid, "type": "private"}, "from": {"id": uid, "is_bot": False, "first_name": "x"}, "text": text}}


cfg = os.path.join(WORK, "c.json")
json.dump({"token": TOKEN, "users": [42], "api": f"http://127.0.0.1:{PORT}"}, open(cfg, "w")); os.chmod(cfg, 0o600)
proc = subprocess.Popen([BIN, "run", "--config", cfg, "--socket", os.path.join(WORK, "none.sock")],
                        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
fails = []
def check(c, what):
    print(("ok   " if c else "FAIL ") + what); c or fails.append(what)

# 1. Telegram unreachable at start for 6 s
time.sleep(6)
check(proc.poll() is None, "bot keeps running while Telegram is unreachable at start")
srv = serve(); t_up = time.time()
pending.append(msg(42, "/help"))
time.sleep(12)
first = [t for t, m in calls if m == "getMe"]
check(bool(first) and first[0] - t_up < 10, f"reached Telegram {first[0]-t_up:.1f} s after it came up" if first else "never reached Telegram")
check(any(m == "sendMessage" for _, m in calls), "answered /help after the outage")

# 2. outage mid-run for 6 s
srv.shutdown(); srv.server_close(); n_before = len(calls)
time.sleep(6)
srv = serve(); t_up = time.time()
pending.append(msg(42, "/help"))
time.sleep(18)
after = [(t, m) for t, m in calls[n_before:] if m == "sendMessage"]
check(bool(after) and after[0][0] - t_up < 17, f"answered again {after[0][0]-t_up:.1f} s after recovery" if after else "no answer after recovery")

proc.terminate(); out, err = proc.communicate(timeout=10)
check(TOKEN.split(":")[1] not in err, "token never in the log")
print("--- log"); print("\n".join(err.strip().splitlines()[-8:]))
print("all passed" if not fails else f"{len(fails)} failed"); sys.exit(1 if fails else 0)
