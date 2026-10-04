#!/usr/bin/env python3
"""Drives a booting NorOS VM over QMP: waits for the desktop, takes screenshots,
opens the terminal and the launcher like a user would."""

import argparse
import json
import socket
import sys
import time


class Qmp:
    def __init__(self, path):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(path)
        self.file = self.sock.makefile("rw")
        self.file.readline()  # greeting
        self.cmd("qmp_capabilities")

    def cmd(self, name, **args):
        self.file.write(json.dumps({"execute": name, "arguments": args}) + "\n")
        self.file.flush()
        while True:
            reply = json.loads(self.file.readline())
            if "return" in reply or "error" in reply:
                if "error" in reply:
                    print(f"qmp {name}: {reply['error']}", file=sys.stderr)
                return reply

    def screenshot(self, path):
        self.cmd("screendump", filename=path, format="png")
        print(f"screenshot: {path}")

    def keys(self, *names, hold=120):
        self.cmd("send-key", keys=[{"type": "qcode", "data": n} for n in names], **{"hold-time": hold})

    def click(self, x, y, width=1440, height=900):
        """Left-click at screen pixel (x, y) with the absolute-pointer tablet."""
        scale = lambda v, size: int(v * 32767 / (size - 1))
        move = [{"type": "abs", "data": {"axis": "x", "value": scale(x, width)}},
                {"type": "abs", "data": {"axis": "y", "value": scale(y, height)}}]
        self.cmd("input-send-event", events=move)
        time.sleep(0.3)
        for down in (True, False):
            self.cmd("input-send-event", events=[{"type": "btn", "data": {"down": down, "button": "left"}}])
            time.sleep(0.15)

    def type(self, text):
        for ch in text:
            self.keys("spc" if ch == " " else ch)
            time.sleep(0.08)


def wait_for(path, needle, timeout):
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            with open(path, errors="replace") as f:
                if needle in f.read():
                    return True
        except FileNotFoundError:
            pass
        time.sleep(2)
    return False


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--timeout", type=int, default=240)
    p.add_argument("--serial", required=True)
    p.add_argument("--socket", required=True)
    p.add_argument("--out", required=True)
    a = p.parse_args()

    for _ in range(30):
        try:
            qmp = Qmp(a.socket)
            break
        except OSError:
            time.sleep(1)
    else:
        print("could not connect to QEMU monitor")
        return 1

    start = time.time()
    booted = wait_for(a.serial, "fjord: first frame on screen", a.timeout)
    elapsed = time.time() - start
    if not booted:
        print(f"desktop did not appear within {a.timeout}s")
        qmp.screenshot(f"{a.out}/0-timeout.png")
        return 1
    print(f"desktop up after ~{elapsed:.0f}s")

    slow = a.timeout > 300
    settle = 25 if slow else 8
    time.sleep(settle)  # let the shell draw wallpaper, bar and dock
    qmp.screenshot(f"{a.out}/1-desktop.png")

    qmp.keys("meta_l", "ret")
    time.sleep(settle)
    qmp.screenshot(f"{a.out}/2-terminal.png")

    qmp.keys("meta_l", "spc")
    time.sleep(settle)
    qmp.type("about")
    time.sleep(3)
    qmp.screenshot(f"{a.out}/3-launcher.png")

    qmp.keys("ret")
    time.sleep(settle)
    qmp.screenshot(f"{a.out}/4-about.png")

    # Close About and the terminal, then visit each 0.2 app.
    qmp.keys("meta_l", "q")
    time.sleep(2)
    qmp.keys("meta_l", "q")
    time.sleep(2)

    def launch(query):
        qmp.keys("meta_l", "spc")
        time.sleep(settle)
        qmp.type(query)
        time.sleep(2)
        qmp.keys("ret")
        time.sleep(settle * 1.5)

    launch("settings")
    qmp.screenshot(f"{a.out}/5-settings.png")
    # Pick the red "Lingonberry" accent; the desktop should restyle itself live.
    qmp.click(928, 347)
    time.sleep(settle)
    qmp.keys("meta_l", "q")
    time.sleep(2)

    qmp.keys("meta_l", "e")
    time.sleep(settle * 1.5)
    qmp.screenshot(f"{a.out}/6-files.png")
    qmp.keys("meta_l", "q")
    time.sleep(2)

    launch("text editor")
    qmp.type("hei fra noros")
    time.sleep(2)
    qmp.screenshot(f"{a.out}/7-text.png")

    # The launcher's highlight uses the accent: it should now be red.
    qmp.keys("meta_l", "spc")
    time.sleep(settle)
    qmp.type("files")
    time.sleep(3)
    qmp.screenshot(f"{a.out}/8-accent-live.png")
    return 0


if __name__ == "__main__":
    sys.exit(main())
