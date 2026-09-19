#!/usr/bin/env python3
"""Record a scripted NOIDA session as an asciicast, for the README demo.

    scripts/record-demo.py demo.cast [--repo PATH]
    agg --font-size 15 --theme asciinema --speed 1.25 demo.cast demo.gif

NOIDA is driven in a real pty: the script sends keystrokes on a schedule and
saves everything the program draws, with timings, in asciicast v2 format. That
keeps the recording honest -- it is the actual TUI, not a mock-up -- and
repeatable, which a hand-made screen capture is not.
"""

import argparse
import json
import os
import pty
import select
import shutil
import signal
import struct
import subprocess
import sys
import termios
import time
import fcntl

COLS, ROWS = 132, 34

# (delay before this step, keys to send, what it is for)
# Keys are raw bytes: \x1b followed by a letter is Alt+letter, which is how
# NOIDA reads Alt. The agent really runs, so the waits are generous; idle time
# is squeezed out afterwards by --max-gap rather than being guessed here.
PROMPT = b"Which file and line defines the function that makes a path relative to the repo root? Reply with just path:line."

SCRIPT = [
    (4.0, b"", "let the tree, editor and agent pane settle"),
    (1.0, PROMPT, "type a question to the agent"),
    (1.2, b"\r", "send it"),
    (90.0, b"", "wait for the agent to search and answer with a file:line"),
    (1.5, b"\x1bj", "Alt+j: label every file reference in the answer"),
    (2.5, b"a", "jump to the first label"),
    (3.5, b"", "the file opens in the editor at that line"),
    (1.2, b"\x1bl", "Alt+l: symbols in this file"),
    (3.0, b"\x1b", "close"),
    (1.2, b"\x1bq", "Alt+q: quit"),
    (1.5, b"y", "confirm"),
    (1.5, b"", "done"),
]


def set_size(fd, rows, cols):
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))


def record(command, cwd, out_path, script):
    env = dict(os.environ)
    env.update(TERM="xterm-256color", COLORTERM="truecolor", LINES=str(ROWS), COLUMNS=str(COLS))
    # A recorded run must not read or write the user's real workspace state,
    # and the agent should start as it would for anyone else rather than
    # inheriting the settings of whatever session is doing the recording.
    for key in list(env):
        if key.startswith("CLAUDE_CODE") or key.startswith("NOIDA_"):
            del env[key]

    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(cwd)
        os.execvpe(command[0], command, env)
    set_size(fd, ROWS, COLS)

    events = []
    start = time.time()
    step = iter(script)
    pending = next(step, None)
    due = start + (pending[0] if pending else 0)

    while True:
        now = time.time()
        timeout = max(0.0, due - now) if pending else 0.2
        ready, _, _ = select.select([fd], [], [], timeout)
        if ready:
            try:
                data = os.read(fd, 65536)
            except OSError:
                break
            if not data:
                break
            events.append([round(time.time() - start, 4), "o", data.decode("utf-8", "replace")])
        if pending and time.time() >= due:
            if pending[1]:
                os.write(fd, pending[1])
            pending = next(step, None)
            if pending is None:
                # Drain whatever the program still draws, then stop.
                deadline = time.time() + 2.0
                while time.time() < deadline:
                    ready, _, _ = select.select([fd], [], [], 0.2)
                    if not ready:
                        continue
                    try:
                        data = os.read(fd, 65536)
                    except OSError:
                        break
                    if not data:
                        break
                    events.append([round(time.time() - start, 4), "o", data.decode("utf-8", "replace")])
                break
            due = time.time() + pending[0]

    try:
        os.kill(pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    os.waitpid(pid, 0)

    return events


def squeeze(events, max_gap):
    """Cap idle time between frames.

    The agent takes as long as it takes, but nobody wants to watch a spinner
    for thirty seconds. Capping each gap keeps every frame the program actually
    drew, in order, and only shortens the waiting.
    """
    out, shift, prev = [], 0.0, 0.0
    for t, kind, data in events:
        gap = t - prev
        if gap > max_gap:
            shift += gap - max_gap
        prev = t
        out.append([round(t - shift, 4), kind, data])
    return out


def write_cast(events, out_path, start):
    header = {"version": 2, "width": COLS, "height": ROWS, "timestamp": int(start), "env": {"TERM": "xterm-256color", "SHELL": "/bin/bash"}}
    with open(out_path, "w") as f:
        f.write(json.dumps(header) + "\n")
        for e in events:
            f.write(json.dumps(e) + "\n")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out", help="asciicast file to write")
    ap.add_argument("--repo", default=".", help="repository to open in NOIDA")
    ap.add_argument("--bin", default="noida", help="NOIDA binary to record")
    ap.add_argument("--max-gap", type=float, default=0.35, help="longest pause to keep, in seconds")
    args = ap.parse_args()

    if not shutil.which(args.bin) and not os.path.exists(args.bin):
        sys.exit(f"{args.bin} not found")
    start = time.time()
    events = record([args.bin, ".", "--fresh"], os.path.abspath(args.repo), args.out, SCRIPT)
    raw = events[-1][0] if events else 0
    events = squeeze(events, args.max_gap)
    n = len(events)
    print(f"wrote {args.out} ({n} events, {raw:.0f}s recorded -> {events[-1][0]:.0f}s played)")
    write_cast(events, args.out, start)
    print("render with: agg --font-size 15 --theme asciinema --speed 1.25", args.out, args.out.replace(".cast", ".gif"))


if __name__ == "__main__":
    main()
