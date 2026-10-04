#!/usr/bin/env python3
"""Native layout tests inside the disposable 1280×900 Sway D-Bus session.

Build Review; export_selection_fixture, export_links_fixture and
export_wayland_fixture into the fixture directory; compile wayland-pointer.c.
Usage: reading-layouts.py binary fixtures pointer [screenshots]
Only internal links are activated. No external URL or print job is dispatched.
"""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

import pyatspi

binary, fixtures, pointer = map(lambda p: str(Path(p).resolve()), sys.argv[1:4])
process = None


def wait(check, description):
    for _ in range(100):
        result = check()
        if result:
            return result
        time.sleep(.1)
    raise AssertionError(f"Timed out: {description}")


def walk(node):
    node.clearCache()
    yield node
    for child in node:
        yield from walk(child)


def named(name, role=None):
    for app in pyatspi.Registry.getDesktop(0):
        if app.get_process_id() == process.pid:
            return next((n for n in walk(app) if n.name == name
                         and (role is None or n.getRoleName() == role)), None)
    return None


def find(name, role=None):
    return wait(lambda: named(name, role), name)


def click(name, role="push button"):
    if name in {"Rotate left", "Rotate right", "Select text", "Hand tool"} and not named(name, role):
        click("Reading options")
    if name == "Annotations" and not named(name, role):
        click("Tools")
    action = find(name, role).queryAction()
    assert action.nActions and action.doAction(0), name
    time.sleep(.4)


def key(name, *modifiers):
    args = ["wtype", "-s", "100"]
    for modifier in modifiers:
        args += ["-M", modifier]
    args += ["-k", name]
    for modifier in reversed(modifiers):
        args += ["-m", modifier]
    subprocess.run(args + ["-s", "150"], check=True)
    time.sleep(.2)


def window():
    pending = [json.loads(subprocess.check_output(["swaymsg", "-t", "get_tree"]))]
    while pending:
        node = pending.pop()
        if node.get("pid") == process.pid:
            return node
        pending.extend(node.get("nodes", []) + node.get("floating_nodes", []))
    return None


def bounds(page):
    rect = find(f"PDF page {page}", "image").queryComponent().getExtents(pyatspi.WINDOW_COORDS)
    win = window()
    return [win["rect"]["x"] + win["window_rect"]["x"] + rect.x,
            win["rect"]["y"] + win["window_rect"]["y"] + rect.y, rect.width, rect.height]


def point(page, x, y, rotated=False):
    bx, by, width, height = bounds(page)
    if rotated:
        x, y = 1 - y, x
    return [round(bx + x * width), round(by + y * height)]


def mouse(action, *points):
    subprocess.run([pointer, action, *map(str, points)], check=True)
    time.sleep(.2)


def copy(expected):
    key("c", "ctrl")
    wait(lambda: subprocess.check_output(["wl-paste", "--no-newline"], text=True) == expected,
         f"clipboard {expected!r}")
    print(f"PASS: clipboard {expected!r}", flush=True)


def capture(name):
    if len(sys.argv) > 4:
        directory = Path(sys.argv[4])
        directory.mkdir(parents=True, exist_ok=True)
        time.sleep(.4)
        subprocess.run(["grim", str(directory / f"{name}.png")], check=True)


def layout(name):
    click("Reading options")
    click(name)


def zoom(percent):
    key("l", "ctrl")
    subprocess.run(["wtype", str(percent), "-k", "Return", "-s", "200", "-k", "Escape"], check=True)
    time.sleep(.6)


def open_pdf(name):
    global process
    if process:
        process.terminate()
        process.wait(timeout=10)
    process = subprocess.Popen([binary, f"{fixtures}/{name}.pdf"],
                               env={**{k: v for k, v in os.environ.items() if k != "DISPLAY"},
                                    "XDG_STATE_HOME": state_home})
    find("Reading options", "push button")
    assert window()["shell"] == "xdg_shell"
    time.sleep(.8)
    click("Sidebar")


for property_name in ["IsEnabled", "ScreenReaderEnabled"]:
    subprocess.run(["gdbus", "call", "--session", "--dest", "org.a11y.Bus",
                    "--object-path", "/org/a11y/bus", "--method",
                    "org.freedesktop.DBus.Properties.Set", "org.a11y.Status",
                    property_name, "<true>"], check=True, stdout=subprocess.DEVNULL)

with tempfile.TemporaryDirectory(prefix="review-layouts-state-") as state_home, \
     subprocess.Popen(["wtype", "-s", "240000"]) as keyboard:
    try:
        open_pdf("selection")
        layout("Facing pages")
        expected = "Left first\nLeft second\n\nRight first\nRight second\n\nRotated\nLast"
        for rotated in [False, True]:
            if rotated:
                click("Rotate right")
                a, b = bounds(1), bounds(2)
                assert abs(a[2] / a[3] - 4 / 3) < .02
                assert abs(b[2] / b[3] - 1 / 2) < .02
            a = point(1, 31 / 300, 64 / 400, rotated)
            b = point(2, 61 / 400, 74 / 200, rotated)
            mouse("drag", *a, *b)
            copy(expected)
            mouse("drag", *b, *a)
            copy(expected)
            capture("facing-rotated-selection" if rotated else "facing-selection")
        print("PASS: facing geometry and bidirectional cross-page selection before/after rotation", flush=True)

        zoom(150)
        # Choose an interior pointer position, independent of glyph/column order.
        p = [450, 350]
        before = bounds(1)
        normalized = [(p[0] - before[0]) / before[2], (p[1] - before[1]) / before[3]]
        with subprocess.Popen(["wtype", "-M", "ctrl", "-s", "2200", "-m", "ctrl", "-s", "100"]) as control:
            time.sleep(.3)
            mouse("scroll", *p, -100)
            control.wait(timeout=5)
        time.sleep(.6)
        after = bounds(1)
        restored = [(p[0] - after[0]) / after[2], (p[1] - after[1]) / after[3]]
        assert all(abs(a - b) < .004 for a, b in zip(normalized, restored)), (normalized, restored)
        assert after[2] > before[2], (before, after)
        click("Hand tool")
        before = bounds(1)
        mouse("drag", *p, p[0] + 61, p[1] - 87)
        after = bounds(1)
        assert abs(after[0] - before[0] - 61) <= 2, (before, after)
        assert abs(after[1] - before[1] + 87) <= 2, (before, after)
        capture("pointer-zoom-hand-pan")
        print("PASS: native pointer-centred Ctrl+wheel zoom and asymmetric hand pan", flush=True)
        key("F11")
        wait(lambda: window()["fullscreen_mode"] == 1, "native fullscreen")
        capture("fullscreen")
        key("F11")
        wait(lambda: window()["fullscreen_mode"] == 0, "leave fullscreen")
        print("PASS: native fullscreen toggle", flush=True)

        click("Rotate left")
        click("Select text")
        layout("Continuous")
        zoom(75)
        mouse("drag", *point(1, 31 / 300, 64 / 400), *point(2, 61 / 400, 74 / 200))
        copy(expected)
        capture("continuous-selection")
        zoom(150)
        mouse("scroll", 600, 400, 600)
        wait(lambda: find("Page", "entry").queryText().getText(0, -1) == "2", "scroll updates active page")
        key("T", "ctrl", "shift")
        assert find("Page 2 text", "entry").queryText().getText(0, -1) == "Last alpha"
        capture("continuous-page-text")
        print("PASS: continuous scrolling, cross-page copy and active-page assistive text", flush=True)

        open_pdf("restricted")
        layout("Facing pages")
        subprocess.run(["wl-copy", "permission sentinel"], check=True)
        key("a", "ctrl")
        copy("permission sentinel")
        mouse("drag", *point(1, .1, .16), *point(2, .2, .37))
        copy("permission sentinel")
        capture("restricted-facing")
        print("PASS: copy restrictions survive multi-page selection", flush=True)

        open_pdf("links")
        layout("Continuous")
        click("Rotate right")
        key("g", "ctrl")
        subprocess.run(["wtype", "1", "-k", "Return", "-s", "200", "-k", "Escape"], check=True)
        time.sleep(.5)
        capture("rotated-link-source")
        p = point(1, 100 / 300, 60 / 400, True)
        with subprocess.Popen([pointer, "move", *map(str, p), "2500"]) as hover:
            time.sleep(1.1)
            capture("rotated-link-hover")
            hover.wait(timeout=5)
        source = bounds(1)
        mouse("click", *p)
        wait(lambda: window()["name"].endswith("2/2 — 225%"), "rotated internal destination")
        time.sleep(.8)
        target = bounds(2)
        capture("rotated-link-destination")
        key("Left", "alt")
        wait(lambda: window()["name"].endswith("1/2 — Fit page"), "Back")
        assert bounds(1) == source
        capture("rotated-link-back")
        if len(sys.argv) > 4:
            directory = Path(sys.argv[4])
            x, y, width, height = source
            for name in ["source", "back"]:
                subprocess.run(["magick", str(directory / f"rotated-link-{name}.png"),
                                "-crop", f"{width}x{height}+{x}+{y}", "+repage",
                                str(Path(state_home) / f"{name}.png")], check=True)
            subprocess.run(["magick", "compare", "-metric", "AE",
                            f"{state_home}/source.png", f"{state_home}/back.png", "null:"], check=True)
        key("Right", "alt")
        wait(lambda: window()["name"].endswith("2/2 — 225%"), "Forward")
        assert bounds(2) == target
        print("PASS: rotated internal link, destination zoom and identical restored native page geometry", flush=True)
        layout("Facing pages")
        click("Sidebar")
        click("Fit width destination  ·  Page 2")
        # FitH fits the target's original width, now its vertical screen axis,
        # rather than fitting the entire mixed-size spread (under 400px tall).
        assert 650 < bounds(2)[3] < 820, bounds(2)
        capture("rotated-facing-fit-destination")
        print("PASS: rotated facing outline destination fits its own original page axis", flush=True)

        open_pdf("outline")
        layout("Facing pages")
        click("Rotate right")
        click("Annotations")
        click("Note")
        mouse("click", *point(2, .27, .63, True))
        wait(lambda: "outline.pdf *" in window()["name"], "rotated note on second page")
        assert find("Page", "entry").queryText().getText(0, -1) == "2"
        find("Note · (no comment)", "push button")
        time.sleep(.8)
        capture("rotated-facing-note")
        key("z", "ctrl")
        wait(lambda: "outline.pdf *" not in window()["name"], "annotation Undo")
        key("y", "ctrl")
        wait(lambda: "outline.pdf *" in window()["name"], "annotation Redo")
        click("Ink")
        mouse("drag", *point(2, .2, .2, True), *point(2, .47, .6, True))
        find("Ink · (no comment)", "push button")
        time.sleep(.8)
        capture("rotated-facing-ink")
        click("Select")
        mouse("click", *point(2, .29, .65, True))
        capture("rotated-facing-annotation-selection")
        print("PASS: rotated facing note/ink on second page, current-page panel and Undo/Redo", flush=True)
    finally:
        if process and process.poll() is None:
            process.terminate()
            process.wait(timeout=10)
        keyboard.terminate()
        keyboard.wait(timeout=10)
