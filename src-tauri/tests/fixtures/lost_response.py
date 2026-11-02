"""Drop one response around a real isolated CLI; keep its original owner alive.

The Rust test releases this fixture explicitly, including on assertion failure.
No mutation is repeated and no server is detached from the test's lifetime.
"""
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import threading

cli, base, mode, release = sys.argv[1:]
server = subprocess.Popen(
    [cli, "--base-dir", base, "serve", "--stdio"],
    stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
)
try:
    sys.stdout.buffer.write(server.stdout.readline())
    sys.stdout.buffer.flush()
    request = sys.stdin.buffer.readline()
    if mode != "before_admission":
        server.stdin.write(request)
        server.stdin.flush()
        for line in server.stdout:
            message = json.loads(line)
            snapshot = message.get("payload", {})
            if not isinstance(snapshot, dict):
                snapshot = {}
            phases = [item["phase"] for item in snapshot.get("items", [])]
            if (mode == "after_admission" and message.get("id") == 1
                or mode == "applying" and "applying" in phases
                or mode == "committed" and snapshot.get("settled")):
                break
    os.close(sys.stdout.fileno())
    def drain():
        for _ in server.stdout:
            pass
    threading.Thread(target=drain, daemon=True).start()
    while not Path(release).exists():
        time.sleep(0.01)
finally:
    server.stdin.close()
    server.wait(timeout=10)
