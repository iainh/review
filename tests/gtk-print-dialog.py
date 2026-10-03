"""Inspect GTK printing through AT-SPI. Never activate Print or Preview."""

import sys
import time
from pathlib import Path

import pyatspi


def walk(node):
    yield node
    for child in node:
        yield from walk(child)


operation, marker = sys.argv[1:]
dialog = next(
    node
    for app in pyatspi.Registry.getDesktop(0)
    for node in walk(app)
    if node.name == "Print" and node.getRole() == pyatspi.ROLE_DIALOG
)
nodes = list(walk(dialog))
if operation == "general":
    names = {node.name for node in nodes}
    assert {"Printer", "All Pages", "Current Page", "Pages:", "Page Setup"} <= names
    pages = next(node for node in nodes if node.name == "Pages:")
    assert pages.queryAction().doAction(0)
    entry = next(
        node for node in nodes if node.name == "Pages" and node.getRole() == pyatspi.ROLE_TEXT
    )
    assert entry.queryEditableText().setTextContents("2")
    assert entry.queryText().getText(0, -1) == "2"
    print("PASS: native printer chooser and one-based page range 2", flush=True)
elif operation == "setup":
    tabs = next(node for node in nodes if node.getRole() == pyatspi.ROLE_PAGE_TAB_LIST)
    assert tabs.querySelection().selectChild(1)
    time.sleep(0.3)
    names = {node.name for node in walk(dialog)}
    assert {"Orientation:", "Portrait", "Landscape", "Two-sided:"} <= names
    print("PASS: native orientation choices and printer-dependent two-sided control", flush=True)
elif operation == "cancel":
    cancel = next(
        node for node in nodes if node.name == "Cancel" and node.getRole() == pyatspi.ROLE_PUSH_BUTTON
    )
    assert cancel.queryAction().doAction(0)
else:
    raise ValueError(operation)
Path(marker).write_text("PASS")
