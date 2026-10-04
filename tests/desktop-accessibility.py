#!/usr/bin/env python3
"""Exercise egui-desktop through native AT-SPI and Wayland, not a browser.

Usage: desktop-accessibility.py binary fixtures screenshots pointer
Run on the disposable Sway session's D-Bus (tests/wayland-desktop.sh).
"""
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


def nodes():
    for app in pyatspi.Registry.getDesktop(0):
        if any(node.name.startswith("Review") for node in walk(app)):
            yield from walk(app)


def named(name, role="push button", enabled=True):
    matches = [node for node in nodes() if node.name == name
               and node.getRoleName() == role
               and (not enabled or node.getState().contains(pyatspi.STATE_ENABLED))]
    # File actions also exist in the disabled document toolbar behind the
    # menu. Dropdown actions span their row; toolbar buttons are compact.
    return max(matches, key=lambda node: node.queryComponent().getExtents(pyatspi.DESKTOP_COORDS).width,
               default=None)


def find(name, role="push button", enabled=True):
    return wait(lambda: named(name, role, enabled), f"{role}: {name}")


def click(name):
    if name == "Forms" and not named(name):
        click("Tools")
    assert find(name).queryAction().doAction(0), name
    time.sleep(.3)


def key(name, *modifiers):
    command = ["wtype", "-s", "150"]
    for modifier in modifiers:
        command += ["-M", modifier]
    command += ["-k", name]
    for modifier in reversed(modifiers):
        command += ["-m", modifier]
    subprocess.run(command + ["-s", "150"], check=True)


def type_text(text):
    subprocess.run(["wtype", "-s", "150", text, "-s", "150"], check=True)


def entry(name):
    return find(name, "entry", enabled=False).queryText().getText(0, -1)


def tree():
    root = json.loads(subprocess.check_output(["swaymsg", "-t", "get_tree"]))

    def descend(node):
        if node.get("app_id") == "review":
            return node
        for child in node.get("nodes", []) + node.get("floating_nodes", []):
            result = descend(child)
            if result:
                return result
        return None

    return descend(root)


binary = str(Path(sys.argv[1]).resolve())
fixtures = Path(sys.argv[2]).resolve()
captures = Path(sys.argv[3]).resolve()
pointer = sys.argv[4]
state_path = fixtures / "state/review/state.json"
environment = {k: v for k, v in os.environ.items() if k != "DISPLAY"}
environment["XDG_STATE_HOME"] = str(fixtures / "state")


def capture(name):
    captures.mkdir(parents=True, exist_ok=True)
    time.sleep(.6)
    subprocess.run(["grim", str(captures / f"desktop-{name}.png")], check=True)


def choose(path):
    wait(lambda: any(app.name in ("zenity", "xdg-desktop-portal-gtk")
                     for app in pyatspi.Registry.getDesktop(0)), "native chooser")
    subprocess.run(["/usr/bin/python3", "tests/gtk-chooser.py", str(path), str(fixtures / "chooser-done")], check=True)
    time.sleep(.5)


def open_pdf(filename):
    click("File")
    click("Open…")
    choose(fixtures / filename)
    wait(lambda: tree() and filename in tree()["name"], f"loaded {filename}")
    find("PDF page 1", "image")


for prop in ["IsEnabled", "ScreenReaderEnabled"]:
    subprocess.run(["gdbus", "call", "--session", "--dest", "org.a11y.Bus",
                    "--object-path", "/org/a11y/bus", "--method",
                    "org.freedesktop.DBus.Properties.Set", "org.a11y.Status", prop,
                    "<true>"], check=True)

with subprocess.Popen(["wtype", "-s", "300000"]) as keyboard, \
     subprocess.Popen([binary], env=environment) as process:
    try:
        find("File")
        assert tree()["shell"] == "xdg_shell"
        for label in ["Recent", "Bookmarks", "View", "Help", "Close window", "Maximize window", "Minimize window"]:
            find(label)
        capture("empty")
        click("File")
        for label in ["Save", "Save As…", "Print…", "Close tab"]:
            node = find(label, enabled=False)
            # The locked accesskit_atspi_common 0.18.1 reports disabled
            # buttons as Enabled/Sensitive. Check the real action gate here;
            # desktop's Rust test checks the underlying disabled node flags.
            action = node.queryAction()
            if action.nActions:
                action.doAction(0)
                time.sleep(.3)
            assert process.poll() is None
            assert not any(app.name == "zenity" for app in pyatspi.Registry.getDesktop(0))
            assert not named("Continue…")
        capture("empty-file-menu")
        key("Escape")
        open_pdf("outline.pdf")
        click("File")
        find("Save")
        find("Print…")
        capture("pdf-file-menu")
        click("Print…")
        find("Continue…")
        capture("print-options")
        key("Right")
        assert entry("Page") == "1"
        click("Cancel")

        # Keyboard menu navigation must invoke the real callback, with no
        # document navigation when arrows belong to the desktop menu.
        key("F2", "ctrl")
        key("Right")
        key("Right")
        key("Right")
        key("Right")
        key("Return")
        key("Return")
        find("Close help")
        assert entry("Page") == "1"
        capture("keyboard-help")
        click("Close help")

        for label, value in [("High contrast", "HighContrast"), ("Light", "Light"), ("Dark", "Dark"), ("System", "System")]:
            click("View")
            click("Appearance")
            capture("appearance-menu" if value == "HighContrast" else f"{value.lower()}-menu")
            click(label)
            wait(lambda: state_path.exists() and json.loads(state_path.read_text())["appearance"] == value, f"persisted {value}")
            if value in ("HighContrast", "Light"):
                capture(value.lower())

        click("View")
        click("Fullscreen")
        wait(lambda: tree()["fullscreen_mode"] != 0, "fullscreen")
        capture("fullscreen")
        key("F11")
        wait(lambda: tree()["fullscreen_mode"] == 0, "leave fullscreen")

        key("g", "ctrl")
        type_text("2")
        key("Return")
        wait(lambda: entry("Page") == "2", "page two")
        key("Left", "alt")
        wait(lambda: entry("Page") == "1", "Alt+Left history is not intercepted")
        key("Right", "alt")
        wait(lambda: entry("Page") == "2", "Alt+Right history is not intercepted")
        key("g", "ctrl")
        key("Left")
        assert entry("Page") == "2"
        key("Escape")
        capture("pdf-page-two")
        print("PASS: native shell labels, menus, keyboard callbacks, themes, fullscreen, history and focused-field gates", flush=True)

        open_pdf("forms.pdf")
        click("Forms")
        assert find("Full name", "entry").queryComponent().grabFocus()
        key("a", "ctrl")
        type_text("Mina desktop")
        wait(lambda: " * — " in tree()["name"], "dirty form title")
        click("Close window")
        find("Cancel")
        capture("dirty-titlebar-close")
        click("Cancel")
        assert process.poll() is None
        assert entry("Full name") == "Mina desktop"
        for action in ["Close tab", "Quit"]:
            click("File")
            click(action)
            find("Cancel")
            click("Cancel")
            assert process.poll() is None
            assert entry("Full name") == "Mina desktop"
        click("File")
        click("Save")
        wait(lambda: " * — " not in tree()["name"], "menu save clears dirty state")
        assert entry("Full name") == "Mina desktop"
        capture("saved-forms")
        click("File")
        click("Close tab")
        wait(lambda: "outline.pdf" in tree()["name"], "close saved tab selects the other document")
        assert process.poll() is None
        open_pdf("forms.pdf")
        click("Forms")
        assert entry("Full name") == "Mina desktop"
        print("PASS: titlebar Close preserves dirty tab, cancellation preserves fields, File/Save uses native document save", flush=True)

        # Narrow floating geometry exercises chrome independent of Sway tiling.
        click("Forms")
        key("F9")
        subprocess.run(["swaymsg", '[app_id="^review$"] floating enable, resize set 480 px 500 px, move position center'], check=True)
        time.sleep(.5)
        find("Close window")
        click("View")
        click("Appearance")
        capture("narrow-menu")
        key("Escape")
        click("Help")
        click("Keyboard shortcuts")
        capture("narrow-help")
        close = find("Close help").queryComponent().getExtents(pyatspi.WINDOW_COORDS)
        window = tree()
        assert close.y >= 0 and close.y + close.height <= window["window_rect"]["height"]
        assert any(node.getRoleName() == "scroll bar" for node in nodes())
        first_row = find("Open PDF", "label")
        initial = first_row.queryComponent().getExtents(pyatspi.WINDOW_COORDS).y
        x = window["rect"]["x"] + window["window_rect"]["x"] + close.x + 120
        y = window["rect"]["y"] + window["window_rect"]["y"] + close.y - 40
        subprocess.run([pointer, "scroll", str(x), str(y), "900"], check=True)
        wait(lambda: first_row.queryComponent().getExtents(pyatspi.WINDOW_COORDS).y < initial,
             "help content scrolls independently")
        assert entry("Page") == "1"
        capture("narrow-help-scrolled")
        click("Close help")
        click("Maximize window")
        for _ in range(30):
            if named("Restore window"):
                break
            time.sleep(.1)
        if named("Restore window"):
            capture("maximized")
            click("Restore window")
            wait(lambda: named("Maximize window"), "restore control state")
            print("PASS: native maximize/restore", flush=True)
        else:
            # Sway does not acknowledge xdg_toplevel maximize requests.
            # Keep the real winit state; never simulate a maximized window.
            print("SKIP: compositor did not acknowledge native maximize; command/state covered by Rust tests", flush=True)
        # Drag and resize must reach native winit rather than stay as ignored
        # viewport commands. Assert geometry differs, not just that input ran.
        before = tree()["rect"]
        subprocess.run([pointer, "drag", str(before["x"] + 250), str(before["y"] + 16),
                        str(before["x"] + 260), str(before["y"] + 16),
                        str(before["x"] + 300), str(before["y"] + 66)], check=True)
        wait(lambda: tree()["rect"]["x"] != before["x"] or tree()["rect"]["y"] != before["y"], "titlebar native drag")
        window = tree()
        before, client = window["rect"], window["window_rect"]
        # Floating Sway borders are outside the client surface. Hit Review's
        # resize handle, not the compositor's outer border.
        right = before["x"] + client["x"] + client["width"]
        bottom = before["y"] + client["y"] + client["height"]
        subprocess.run([pointer, "drag", str(right - 2), str(bottom - 2),
                        str(right - 12), str(bottom - 12),
                        str(right + 70), str(bottom + 60)], check=True)
        wait(lambda: tree()["rect"]["width"] > before["width"], "native edge resize")
        capture("resized")
        click("Close window")
        process.wait(timeout=10)
        assert process.returncode == 0
        print("PASS: native narrow chrome/help, titlebar drag, edge resize and clean shutdown", flush=True)
    finally:
        if process.poll() is None:
            process.terminate()
            process.wait(timeout=10)
        keyboard.terminate()
        keyboard.wait(timeout=10)
