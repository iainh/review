#!/usr/bin/env python3
"""Export a rendered page's screen bounds in the disposable Sway D-Bus session."""
import json
from pathlib import Path
import subprocess
import sys
import time

import pyatspi


def walk(node):
    node.clearCache()
    yield node
    for child in node:
        yield from walk(child)


for name in ["IsEnabled", "ScreenReaderEnabled"]:
    subprocess.run(["gdbus", "call", "--session", "--dest", "org.a11y.Bus",
                    "--object-path", "/org/a11y/bus", "--method",
                    "org.freedesktop.DBus.Properties.Set", "org.a11y.Status",
                    name, "<true>"], check=True, stdout=subprocess.DEVNULL)

for _ in range(100):
    image = next((node for app in pyatspi.Registry.getDesktop(0) for node in walk(app)
                  if node.getRoleName() == "image" and node.name == sys.argv[2]), None)
    if image:
        # Wayland does not expose global window positions to winit. Combine
        # window-local AccessKit bounds with Sway's actual client-area origin.
        tree = json.loads(subprocess.check_output(["swaymsg", "-t", "get_tree"]))
        pending = [tree]
        while pending:
            window = pending.pop()
            if window.get("pid") == image.getApplication().get_process_id():
                rect = image.queryComponent().getExtents(pyatspi.WINDOW_COORDS)
                origin = window["rect"]
                client = window["window_rect"]
                Path(sys.argv[1]).write_text(json.dumps([
                    origin["x"] + client["x"] + rect.x,
                    origin["y"] + client["y"] + rect.y,
                    rect.width, rect.height,
                ]))
                sys.exit(0)
            pending.extend(window.get("nodes", []) + window.get("floating_nodes", []))
    time.sleep(0.1)
raise AssertionError(f"No rendered {sys.argv[2]}")
