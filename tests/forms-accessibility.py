#!/usr/bin/env python3
"""Native forms checks on the disposable Sway session's AT-SPI bus."""
import json
import os
from pathlib import Path
import subprocess
import sys
import time

import pyatspi


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
        for node in walk(app):
            if node.name == name and (role is None or node.getRoleName() == role):
                return node


def find(name, role=None):
    return wait(lambda: named(name, role), name)


def click(name, role=None):
    action = find(name, role).queryAction()
    assert action.nActions and action.doAction(0), name
    time.sleep(.3)


def key(name, *modifiers):
    args = ["wtype", "-s", "150"]
    for modifier in modifiers:
        args += ["-M", modifier]
    args += ["-k", name]
    for modifier in reversed(modifiers):
        args += ["-m", modifier]
    subprocess.run(args + ["-s", "150"], check=True)
    time.sleep(.2)


def entry(name):
    return find(name, "entry").queryText().getText(0, -1)


def fill(name, value):
    node = find(name, "entry")
    assert node.queryComponent().grabFocus()
    time.sleep(.2)
    key("a", "ctrl")
    subprocess.run(["wtype", "-s", "150", value, "-s", "200"], check=True)
    wait(lambda: entry(name) == value, f"value {name}")


def page_point(page, x, y):
    rect = find(f"PDF page {page}", "image").queryComponent().getExtents(pyatspi.WINDOW_COORDS)
    pending = [json.loads(subprocess.check_output(["swaymsg", "-t", "get_tree"]))]
    while pending:
        window = pending.pop()
        if window.get("pid") == process.pid:
            return [round(window["rect"]["x"] + window["window_rect"]["x"] + rect.x + x * rect.width),
                    round(window["rect"]["y"] + window["window_rect"]["y"] + rect.y + y * rect.height)]
        pending.extend(window.get("nodes", []) + window.get("floating_nodes", []))
    raise AssertionError("Review window not found")


def capture(name):
    if len(sys.argv) > 3:
        path = Path(sys.argv[3])
        path.mkdir(parents=True, exist_ok=True)
        wait(lambda: named("PDF page 1", "image"), "rendered page")
        time.sleep(.5)
        subprocess.run(["grim", str(path / f"forms-{name}.png")], check=True)


for prop in ["IsEnabled", "ScreenReaderEnabled"]:
    subprocess.run(["gdbus", "call", "--session", "--dest", "org.a11y.Bus",
                    "--object-path", "/org/a11y/bus", "--method",
                    "org.freedesktop.DBus.Properties.Set", "org.a11y.Status", prop,
                    "<true>"], check=True)

binary = str(Path(sys.argv[1]).resolve())
fixtures = Path(sys.argv[2]).resolve()
environment = {k: v for k, v in os.environ.items() if k != "DISPLAY"}
environment["XDG_STATE_HOME"] = str(fixtures / "state")
with subprocess.Popen(["wtype", "-s", "120000"]) as keyboard:
    try:
        for filename in ["forms", "forms-only", "forms-denied"]:
            with subprocess.Popen([binary, str(fixtures / f"{filename}.pdf")], env=environment) as process:
                try:
                    click("Forms", "push button")
                    if filename == "forms-denied":
                        field = find("Full name · read-only", "entry")
                        assert not field.getState().contains(pyatspi.STATE_ENABLED)
                        capture("denied")
                        key("w", "ctrl")
                        process.wait(timeout=10)
                        print("PASS: native permission-denied fields are disabled", flush=True)
                        continue
                    if filename == "forms-only":
                        assert find("Full name", "entry").queryComponent().grabFocus()
                        subprocess.run(["wl-copy", "form copy sentinel"], check=True)
                        key("a", "ctrl")
                        key("c", "ctrl")
                        assert subprocess.check_output(["wl-paste", "--no-newline"]).decode() == "form copy sentinel"
                    fill("Full name", "Nora native")
                    assert find("Full name", "entry").getState().contains(pyatspi.STATE_FOCUSED)
                    key("Tab")
                    assert find("Consent", "check box").getState().contains(pyatspi.STATE_FOCUSED)
                    key("space")
                    wait(lambda: find("Consent", "check box").getState().contains(pyatspi.STATE_CHECKED), "checked consent")
                    key("Tab", "shift")
                    assert find("Full name", "entry").getState().contains(pyatspi.STATE_FOCUSED)
                    key("Tab")
                    key("Tab")
                    assert find("Plan: Alpha", "radio button").getState().contains(pyatspi.STATE_FOCUSED)
                    key("space")
                    key("Tab")
                    assert find("Country", "combo box").getState().contains(pyatspi.STATE_FOCUSED)
                    key("space")
                    assert find("Cameroon", "push button").queryComponent().grabFocus()
                    key("space")
                    wait(lambda: named("Cameroon", "push button") is None, "choice menu closes")
                    assert find("Duplicate blue", "push button").queryComponent().grabFocus()
                    key("space")
                    readonly = find("Read-only value · read-only", "entry")
                    assert not readonly.getState().contains(pyatspi.STATE_ENABLED)
                    assert readonly.queryText().getText(0, -1) == "Protected"
                    capture("filled" if filename == "forms" else "fill-only")
                    fill("Message", "Native first\nNative second")
                    # Page shortcuts must not navigate while a form entry owns focus.
                    key("Right")
                    assert entry("Page") == "1"
                    fill("Custom choice", "Native custom")
                    capture("multiline")
                    key("w", "ctrl")
                    find("Cancel", "push button")
                    capture("unsaved")
                    click("Cancel", "push button")
                    if filename == "forms":
                        click("Close", "push button")
                        click("Layout", "combo box")
                        click("Facing pages", "push button")
                        click("Rotate right", "push button")
                        wait(lambda: named("PDF page 2", "image"), "facing page two")
                        time.sleep(.5)
                        # Page 2 text centre is (.625, .4375) in original PDF
                        # coordinates: clockwise rotation makes it (.5625, .625).
                        position = page_point(2, .5625, .625)
                        subprocess.run([sys.argv[4], "click", *map(str, position)], check=True)
                        wait(lambda: entry("Page") == "2", "page-two field activates its page")
                        wait(lambda: find("Page two note", "entry").getState().contains(pyatspi.STATE_FOCUSED), "page-two field focus")
                        fill("Page two note", "Rotated second page")
                        assert entry("Page") == "2"
                        click("Plan: Beta", "radio button")
                        wait(lambda: find("Plan: Beta", "radio button").getState().contains(pyatspi.STATE_CHECKED), "second-page radio selected")
                        click("Undo", "push button")
                        wait(lambda: not find("Plan: Beta", "radio button").getState().contains(pyatspi.STATE_CHECKED), "radio undo on rotated page")
                        click("Redo", "push button")
                        wait(lambda: find("Plan: Beta", "radio button").getState().contains(pyatspi.STATE_CHECKED), "radio redo on rotated page")
                        capture("facing-rotated-page-two")
                        print("PASS: rotated facing page-two field hit/focus, text entry, cross-page radio and undo/redo", flush=True)
                    key("s", "ctrl")
                    wait(lambda: any(node.name.startswith(f"Review — {filename}.pdf —") for app in pyatspi.Registry.getDesktop(0) for node in walk(app)), "clean saved title")
                    key("w", "ctrl")
                    process.wait(timeout=10)
                    assert process.returncode == 0
                    print(f"PASS: {filename} native keyboard fields, buttons, choices, read-only, dirty-close and saved title", flush=True)
                finally:
                    if process.poll() is None:
                        process.terminate()
                        process.wait(timeout=10)
        state = json.loads((fixtures / "state/review/state.json").read_text())
        assert "Nora native" not in json.dumps(state)
        print("PASS: form contents are not stored in session metadata", flush=True)
    finally:
        keyboard.terminate()
        keyboard.wait(timeout=10)
