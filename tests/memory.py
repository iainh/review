#!/usr/bin/python3
"""Repeatable native Wayland memory benchmark (release binary, PDF, output dir).

Run on the disposable Sway session's D-Bus, with tests/wayland-pointer.c built
as REVIEW_POINTER. Each repetition uses a fresh process and private state/PDF.
Measures the Review process only; GPU allocations and compositor are excluded.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import threading
import time

import pyatspi


def wait(check, description, timeout=180):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        result = check()
        if result:
            return result
        time.sleep(.2)
    raise AssertionError(f"Timed out: {description}")


def walk(node):
    node.clearCache()
    yield node
    for child in node:
        yield from walk(child)


def key(name, *modifiers):
    args = ["wtype", "-s", "150"]
    for modifier in modifiers:
        args += ["-M", modifier]
    args += ["-k", name]
    for modifier in reversed(modifiers):
        args += ["-m", modifier]
    subprocess.run(args + ["-s", "150"], check=True)


def type_text(text):
    subprocess.run(["wtype", "-s", "150", text, "-s", "150"], check=True)


def submit(shortcut, text):
    # Keep one virtual keyboard alive through focus, text and Enter.
    subprocess.run(["wtype", "-s", "150", "-M", "ctrl", "-k", shortcut,
                    "-m", "ctrl", "-s", "150", text, "-s", "150",
                    "-k", "Return", "-s", "150"], check=True)


binary, pdf, output = map(lambda p: Path(p).resolve(), sys.argv[1:4])
output.mkdir(parents=True, exist_ok=True)
repetitions = int(sys.argv[4]) if len(sys.argv) > 4 else 3
for prop in ["IsEnabled", "ScreenReaderEnabled"]:
    subprocess.run(["gdbus", "call", "--session", "--dest", "org.a11y.Bus",
                    "--object-path", "/org/a11y/bus", "--method",
                    "org.freedesktop.DBus.Properties.Set", "org.a11y.Status",
                    prop, "<true>"], check=True, stdout=subprocess.DEVNULL)

report = {"binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
          "pdf_sha256": hashlib.sha256(pdf.read_bytes()).hexdigest(),
          "pdf_bytes": pdf.stat().st_size, "runs": []}

for repetition in range(repetitions):
    with tempfile.TemporaryDirectory(prefix="review-memory-") as scratch:
        document = Path(scratch) / "benchmark.pdf"
        shutil.copyfile(pdf, document)
        log = (output / f"run-{repetition + 1}.log").open("w")
        env = {**os.environ, "XDG_STATE_HOME": scratch + "/state",
               "WINIT_UNIX_BACKEND": "wayland"}
        env.pop("DISPLAY", None)
        process = subprocess.Popen([binary, document], env=env, stdout=log, stderr=log)
        started = time.monotonic()
        samples, checkpoints = [], []
        stop = threading.Event()

        def sample():
            while not stop.is_set():
                try:
                    values = dict(re.findall(r"^(\w+):\s+(\d+) kB",
                                             Path(f"/proc/{process.pid}/smaps_rollup").read_text(), re.M))
                    status = dict(re.findall(r"^(\w+):\s+(\d+) kB",
                                             Path(f"/proc/{process.pid}/status").read_text(), re.M))
                    samples.append({"seconds": time.monotonic() - started,
                                    **{k: int(values[k]) for k in ["Rss", "Pss", "Private_Clean", "Private_Dirty"]},
                                    "VmHWM": int(status["VmHWM"])})
                except (FileNotFoundError, ProcessLookupError):
                    break
                stop.wait(.1)

        monitor = threading.Thread(target=sample)
        monitor.start()

        def nodes():
            for app in pyatspi.Registry.getDesktop(0):
                if app.get_process_id() == process.pid:
                    yield from walk(app)

        def named(name):
            return next((n for n in nodes() if n.name == name), None)

        def click(name):
            node = wait(lambda: named(name), name)
            assert node.queryAction().doAction(0), name
            time.sleep(.4)

        def title():
            tree = json.loads(subprocess.check_output(["swaymsg", "-t", "get_tree"]))
            pending = [tree]
            while pending:
                node = pending.pop()
                if node.get("pid") == process.pid:
                    return node.get("name", "")
                pending.extend(node.get("nodes", []) + node.get("floating_nodes", []))
            return ""

        def checkpoint(name):
            # Identical settling and sample window for every build.
            time.sleep(2)
            since = time.monotonic() - started
            time.sleep(1)
            window = [s for s in samples if s["seconds"] >= since]
            assert window, name
            item = {"stage": name, "title": title(),
                    "seconds": time.monotonic() - started,
                    "rss_kib": statistics.median(s["Rss"] for s in window),
                    "pss_kib": statistics.median(s["Pss"] for s in window),
                    "private_kib": statistics.median(s["Private_Clean"] + s["Private_Dirty"] for s in window),
                    "peak_rss_kib": max(s["VmHWM"] for s in samples)}
            checkpoints.append(item)
            print(json.dumps(item), flush=True)
            if repetition == 0:
                subprocess.run(["grim", str(output / f"{name}.png")], check=True)

        def goto(page):
            submit("g", str(page))
            wait(lambda: f" — {page}/" in title(), f"page {page}")

        try:
            wait(lambda: " — 1/" in title(), "document opened")
            wait(lambda: named("PDF page 1"), "accessible page")
            checkpoint("opened")
            for page in [5, 10, 20, 30, 40, 1]:
                goto(page)
                time.sleep(1)
            checkpoint("browsed")
            submit("l", "200")
            wait(lambda: title().endswith("200%"), "200% zoom")
            checkpoint("zoomed")
            key("Escape")
            key("0")
            submit("f", "map")
            result = wait(lambda: next((n.name for n in nodes()
                                       if re.match(r"\d+ / \d+ matches$", n.name)
                                       or n.name.startswith("Search failed:")), None), "search settled")
            checkpoint("searched")
            key("Escape")
            goto(10)
            # Notes work on scanned and native-text pages alike.
            if not named("Annotations"):
                click("Tools")
            click("Annotations")
            click("Note")
            for x in [500]:
                subprocess.run([os.environ["REVIEW_POINTER"], "click", str(x), "500"], check=True)
                wait(lambda: "benchmark.pdf *" in title(), "unsaved annotation")
                time.sleep(.5)
            checkpoint("edited")
            key("z", "ctrl")
            wait(lambda: "benchmark.pdf *" not in title(), "undo to original")
            checkpoint("undone")
            key("z", "ctrl", "shift")
            wait(lambda: "benchmark.pdf *" in title(), "redo")
            key("s", "ctrl")
            wait(lambda: "benchmark.pdf *" not in title(), "save")
            checkpoint("saved")
            key("w", "ctrl")
            wait(lambda: "benchmark.pdf" not in title(), "closed document")
            checkpoint("closed")
            report["runs"].append({"checkpoints": checkpoints, "search_result": result,
                                   "samples": samples})
        finally:
            process.terminate()
            process.wait(timeout=15)
            stop.set()
            monitor.join()
            log.close()
            (output / "results.json").write_text(json.dumps(report, indent=2) + "\n")

print("PASS: native open, browse, zoom, search, edit, undo, redo, save and close", flush=True)
