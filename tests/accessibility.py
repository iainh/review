#!/usr/bin/env python3
"""Native AT-SPI and keyboard checks. Run inside the disposable Sway D-Bus session.

Build Review and export the outline fixture first. Usage:
  /usr/bin/python3 tests/accessibility.py target/debug/review outline.pdf [screenshots]
Requires python3-pyatspi, gdbus, wtype and grim. No browser or HTML UI is involved.
"""
import os
import json
from pathlib import Path
import subprocess
import sys
import time

import pyatspi


def wait_for(check, description):
    for _ in range(100):
        result = check()
        if result:
            return result
        time.sleep(0.1)
    raise AssertionError(f"Timed out: {description}")


def walk(node):
    # This synchronous test does not run the registry's event loop. Read live
    # names/states instead of depending on asynchronously invalidated caches.
    node.clearCache()
    yield node
    for child in node:
        yield from walk(child)


def application():
    for app in pyatspi.Registry.getDesktop(0):
        if any(node.name.startswith("Review") for node in walk(app)):
            return app
    return None


def named(name, role=None):
    app = application()
    if app:
        return next((node for node in walk(app) if node.name == name
                     and (role is None or node.getRoleName() == role)), None)
    return None


def find(name, role=None):
    return wait_for(lambda: named(name, role), name)


def click(name, role=None):
    action = find(name, role).queryAction()
    assert action.nActions > 0, f"No native action for {name}"
    assert action.doAction(0), f"Native action failed: {name}"
    time.sleep(0.3)


def key(name, *modifiers):
    args = ["wtype", "-s", "150"]
    for modifier in modifiers:
        args += ["-M", modifier]
    args += ["-k", name]
    for modifier in reversed(modifiers):
        args += ["-m", modifier]
    subprocess.run(args + ["-s", "150"], check=True)
    time.sleep(0.2)


def capture(name):
    if len(sys.argv) > 3:
        directory = Path(sys.argv[3])
        directory.mkdir(parents=True, exist_ok=True)
        time.sleep(0.5)
        subprocess.run(["grim", str(directory / f"{name}.png")], check=True)


def page_number():
    return find("Page", "entry").queryText().getText(0, -1)


# Mark an assistive technology as active before creating the native adapter.
for property_name in ["IsEnabled", "ScreenReaderEnabled"]:
    subprocess.run(["gdbus", "call", "--session", "--dest", "org.a11y.Bus",
                    "--object-path", "/org/a11y/bus", "--method",
                    "org.freedesktop.DBus.Properties.Set", "org.a11y.Status",
                    property_name, "<true>"], check=True)

# A headless seat has no physical keyboard. Keep one virtual device alive;
# otherwise destroying each wtype device correctly removes native window focus.
with subprocess.Popen(["wtype", "-s", "120000"]) as keyboard, \
     subprocess.Popen([str(Path(sys.argv[1]).resolve()), str(Path(sys.argv[2]).resolve())],
                      env={key: value for key, value in os.environ.items() if key != "DISPLAY"}) as process:
    try:
        find("Zoom in", "push button")
        find("Zoom out", "push button")
        find("Collapse Chapter one", "push button")
        containers = [json.loads(subprocess.check_output(["swaymsg", "-t", "get_tree"]))]
        windows = []
        while containers:
            node = containers.pop()
            if node.get("pid") == process.pid:
                windows.append(node)
            containers.extend(node.get("nodes", []) + node.get("floating_nodes", []))
        assert len(windows) == 1 and windows[0]["shell"] == "xdg_shell"
        assert page_number() == "1"
        capture("system")
        print("PASS: native AT-SPI tree, labelled controls and outline action", flush=True)

        # The bridge must forward actions, not only publish a static tree.
        click("Next", "push button")
        wait_for(lambda: page_number() == "2", "native next-page action")
        click("Previous", "push button")
        wait_for(lambda: page_number() == "1", "native previous-page action")
        key("g", "ctrl")
        page = find("Page", "entry")
        capture("page-focus")
        assert page.getState().contains(pyatspi.STATE_FOCUSED)
        key("Tab")
        assert find("Go", "push button").getState().contains(pyatspi.STATE_FOCUSED)
        key("space")
        assert page_number() == "1"
        key("Tab", "shift")
        assert find("Page", "entry").getState().contains(pyatspi.STATE_FOCUSED)
        key("Right")
        assert page_number() == "1"
        key("F1")
        find("Close help", "push button")
        key("Right")
        assert page_number() == "1"
        capture("shortcut-help")
        key("Escape")
        wait_for(lambda: find("Page", "entry").getState().contains(pyatspi.STATE_FOCUSED),
                 "focus restored after help")
        key("Escape")
        assert process.poll() is None
        print("PASS: keyboard focus, help isolation, restoration and non-quitting Escape", flush=True)

        key("T", "ctrl", "shift")
        text = find("Page 1 text", "entry")
        assert text.queryText().getText(0, -1).strip() == "Chapter one"
        assert not text.getState().contains(pyatspi.STATE_EDITABLE)
        assert text.getState().contains(pyatspi.STATE_FOCUSED)
        capture("page-text")
        key("F6")
        assert find("Page", "entry").getState().contains(pyatspi.STATE_FOCUSED)
        key("F6", "shift")
        assert find("Page 1 text", "entry").getState().contains(pyatspi.STATE_FOCUSED)
        click("Next", "push button")
        text = find("Page 2 text", "entry")
        assert text.queryText().getText(0, -1).strip() == "Last alpha"
        print("PASS: native read-only document text, page updates and F6 cycle", flush=True)

        for choice, screenshot in [("Light", "light"), ("Dark", "dark"),
                                   ("High contrast", "high-contrast"), ("System", "system-restored")]:
            click("Appearance", "combo box")
            click(choice, "push button")
            capture(screenshot)
        key("F1")
        find("Close help", "push button")
        capture("help-restored")
        key("Escape")
        key("w", "ctrl")
        process.wait(timeout=10)
        assert process.returncode == 0, f"Review exited with status {process.returncode}"
        print("PASS: all appearance choices and Ctrl+W", flush=True)
    finally:
        if process.poll() is None:
            process.terminate()
            process.wait(timeout=10)
        keyboard.terminate()
        keyboard.wait(timeout=10)
