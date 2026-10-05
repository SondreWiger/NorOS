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

    def keys(self, *names, hold=60):
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

    SYMBOLS = {" ": ["spc"], "-": ["minus"], "_": ["shift", "minus"], "/": ["slash"], ".": ["dot"],
               "\n": ["ret"], ":": ["shift", "semicolon"], "=": ["equal"], ">": ["shift", "dot"],
               "&": ["shift", "7"], ";": ["semicolon"], "$": ["shift", "4"], "|": ["shift", "backslash"],
               "'": ["apostrophe"], '"': ["shift", "apostrophe"], "*": ["shift", "8"], "(": ["shift", "9"],
               ")": ["shift", "0"], ",": ["comma"]}

    def type(self, text):
        for ch in text:
            if ch in self.SYMBOLS:
                keys = self.SYMBOLS[ch]
            elif ch.isupper():
                keys = ["shift", ch.lower()]
            else:
                keys = [ch]
            self.keys(*keys)
            time.sleep(0.08)


def wait_for(path, needle, timeout, also=None):
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            with open(path, errors="replace") as f:
                text = f.read()
                if needle in text:
                    return True
                if also and also in text:
                    return False
        except FileNotFoundError:
            pass
        time.sleep(2)
    return False


PASSPHRASE = "nordlys-test-1"


def connect(path):
    for _ in range(30):
        try:
            return Qmp(path)
        except OSError:
            time.sleep(1)
    return None


def wait_desktop(qmp, a, name):
    start = time.time()
    if not wait_for(a.serial, "fjord: first frame on screen", a.timeout):
        print(f"desktop did not appear within {a.timeout}s")
        qmp.screenshot(f"{a.out}/{name}-timeout.png")
        return False
    print(f"desktop up after ~{time.time() - start:.0f}s")
    return True


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--phase", default="live", choices=["live", "install", "installed", "quit", "debug"])
    p.add_argument("--timeout", type=int, default=240)
    p.add_argument("--serial", required=True)
    p.add_argument("--socket", required=True)
    p.add_argument("--out", required=True)
    a = p.parse_args()

    qmp = connect(a.socket)
    if qmp is None:
        print("could not connect to QEMU monitor")
        return 0 if a.phase == "quit" else 1
    if a.phase == "quit":
        qmp.cmd("quit")
        return 0
    settle = 25 if a.timeout > 300 else 8
    return {"live": phase_live, "install": phase_install, "installed": phase_installed, "debug": phase_debug}[a.phase](qmp, a, settle)


def phase_install(qmp, a, settle):
    """Open the installer, then install from a terminal onto the blank disk (/dev/vda)."""
    if not wait_desktop(qmp, a, "i0"):
        return 1
    time.sleep(settle)
    qmp.screenshot(f"{a.out}/i1-live-desktop.png")
    qmp.keys("meta_l", "spc")
    time.sleep(settle)
    qmp.type("install")
    time.sleep(2)
    qmp.keys("ret")
    time.sleep(settle * 1.5)
    qmp.screenshot(f"{a.out}/i2-installer.png")
    qmp.keys("meta_l", "q")
    time.sleep(3)

    qmp.keys("meta_l", "ret")
    time.sleep(settle)
    qmp.type("sudo noros-install --yes --disk /dev/vda --user tester --name Tester\n")
    time.sleep(4)
    qmp.type(PASSPHRASE + "\n")
    time.sleep(2)
    qmp.type(PASSPHRASE + "\n")
    install_timeout = a.timeout * 3
    done = wait_for(a.serial, "noros-install: installation complete", install_timeout, also="noros-install: failed")
    time.sleep(3)
    qmp.screenshot(f"{a.out}/i3-installed.png")
    with open(a.serial, errors="replace") as f:
        log = f.read()
    if "noros-install: installation complete" not in log:
        print("installation did not complete")
        return 1
    print("installation complete")
    return 0


def phase_installed(qmp, a, settle):
    """Boot the installed system: unlock the disk, then check the desktop and Settings."""
    start = time.time()
    booted = False
    # The passphrase prompt isn't visible on the serial log; type it until the desktop appears.
    while time.time() - start < a.timeout:
        if wait_for(a.serial, "fjord: first frame on screen", 60 if a.timeout > 300 else 20):
            booted = True
            break
        qmp.screenshot(f"{a.out}/j0-unlock.png")
        qmp.type(PASSPHRASE + "\n")
    if not booted:
        print("installed system did not reach the desktop")
        qmp.screenshot(f"{a.out}/j0-timeout.png")
        return 1
    print(f"installed desktop up after ~{time.time() - start:.0f}s")
    time.sleep(settle)
    qmp.screenshot(f"{a.out}/j1-installed-desktop.png")

    qmp.keys("meta_l", "spc")
    time.sleep(settle)
    qmp.type("settings")
    time.sleep(2)
    qmp.keys("ret")
    time.sleep(settle * 1.5)
    qmp.click(330, 262)  # "Updates & Recovery" in the sidebar
    time.sleep(settle)
    qmp.screenshot(f"{a.out}/j2-updates.png")
    return 0


def phase_debug(qmp, a, settle):
    """Run a command from NOROS_DEBUG_CMD in a terminal and screenshot its output."""
    import os
    if not wait_desktop(qmp, a, "d0"):
        return 1
    time.sleep(settle)
    qmp.keys("meta_l", "ret")
    time.sleep(settle)
    qmp.type(os.environ.get("NOROS_DEBUG_CMD", "uname -a") + "\n")
    time.sleep(int(os.environ.get("NOROS_DEBUG_WAIT", "120")))
    qmp.screenshot(f"{a.out}/d1-debug.png")
    return 0


def phase_live(qmp, a, settle):
    if not wait_desktop(qmp, a, "0"):
        return 1
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
    qmp.keys("esc")
    time.sleep(2)

    # Vakt: a program in the terminal tries to go online; the user is asked.
    qmp.keys("meta_l", "ret")
    time.sleep(settle)
    qmp.type("echo hei > /dev/tcp/1.1.1.1/80\n")
    time.sleep(settle)
    qmp.screenshot(f"{a.out}/9-vakt-prompt.png")
    time.sleep(65)  # unanswered questions are blocked after 60 s

    qmp.keys("meta_l", "spc")
    time.sleep(settle)
    qmp.type("privacy")
    time.sleep(2)
    qmp.keys("ret")
    time.sleep(settle * 1.5)
    qmp.screenshot(f"{a.out}/10-privacy.png")
    qmp.click(382, 236)  # "Network Activity" in the sidebar
    time.sleep(settle)
    qmp.screenshot(f"{a.out}/11-privacy-network.png")
    qmp.keys("meta_l", "q")
    time.sleep(3)

    # Web: open the browser, load a page (Vakt asks first), then download a file.
    qmp.keys("meta_l", "spc")
    time.sleep(settle)
    qmp.type("web")
    time.sleep(2)
    qmp.keys("ret")
    time.sleep(settle * 5)  # the browser starts several processes; slow under emulation
    qmp.screenshot(f"{a.out}/12-web-start.png")
    qmp.type("example.com\n")
    time.sleep(settle)
    qmp.screenshot(f"{a.out}/13-web-prompt.png")
    qmp.click(918, 229)  # "Always Allow"
    time.sleep(settle * 3)  # the page should now open by itself
    qmp.screenshot(f"{a.out}/14-web-page.png")
    qmp.click(775, 118)  # the address bar
    time.sleep(1)
    qmp.keys("ctrl", "a")
    time.sleep(1)
    qmp.type("https://httpbin.org/bytes/4096\n")
    time.sleep(settle * 3)
    qmp.screenshot(f"{a.out}/15-web-download.png")
    return 0


if __name__ == "__main__":
    sys.exit(main())
