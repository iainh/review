#!/usr/bin/env python3
"""Native OCR controls/text checks; run in Review's disposable Sway D-Bus session."""
import json
from pathlib import Path
import subprocess
import sys
import time

import pyatspi
from gi.repository import GLib


def walk(node):
    try:
        node.clearCache()
        name = node.name
        role = node.getRoleName()
        children = list(node)
    except GLib.Error:
        # A frame can remove a status node while AT-SPI traverses its old ID.
        # Retry live reads; never retry a submitted action.
        return
    yield node, name, role
    for child in children:
        yield from walk(child)


def normalize(text):
    return "".join(text.split()).casefold()


for name in ["IsEnabled", "ScreenReaderEnabled"]:
    subprocess.run(["gdbus", "call", "--session", "--dest", "org.a11y.Bus",
                    "--object-path", "/org/a11y/bus", "--method",
                    "org.freedesktop.DBus.Properties.Set", "org.a11y.Status",
                    name, "<true>"], check=True, stdout=subprocess.DEVNULL)

output, mode, expected = sys.argv[1:]
for _ in range(100):
    for app in pyatspi.Registry.getDesktop(0):
        if not any(name.startswith("Review") for _, name, _ in walk(app)):
            continue
        for node, name, role in walk(app):
            action_mode = mode == "click" and role not in ["label", "entry"]
            action_mode |= mode == "language" and role == "combo box"
            action_mode |= mode == "choose" and role == "push button"
            if action_mode and name == expected:
                action = node.queryAction()
                if action.nActions:
                    assert action.doAction(0), f"Native action failed: {expected}"
                    Path(output).write_text(json.dumps({"action": expected}))
                    sys.exit(0)
            if mode == "status" and normalize(expected) in normalize(name):
                Path(output).write_text(json.dumps({"status": name}))
                sys.exit(0)
            if mode == "pane" and name == "Page 1 text" and role == "entry":
                text = node.queryText().getText(0, -1)
                if text.split() == expected.split():
                    assert not node.getState().contains(pyatspi.STATE_EDITABLE)
                    Path(output).write_text(json.dumps({"read_only_text": text}))
                    sys.exit(0)
    time.sleep(0.1)
raise AssertionError(f"Native OCR check timed out: {mode} {expected}")
