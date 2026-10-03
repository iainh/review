"""Activate native GTK chooser controls through AT-SPI in the test session."""

import sys
import time
from pathlib import Path

import pyatspi


def walk(node):
    yield node
    for child in node:
        yield from walk(child)


def visible(node):
    return node.getState().contains(pyatspi.STATE_SHOWING)


def controls():
    for app in pyatspi.Registry.getDesktop(0):
        if app.name in ("zenity", "xdg-desktop-portal-gtk"):
            yield from walk(app)


operation, marker = sys.argv[1:]
if operation != "cancel":
    chooser = next(
        node
        for node in controls()
        if node.name == "File Chooser Widget" and visible(node)
    )
    assert chooser.queryAction().doAction(0)  # show_location
    time.sleep(0.2)
    entry = next(
        node
        for node in controls()
        if node.getRole() == pyatspi.ROLE_TEXT and visible(node)
    )
    assert entry.queryEditableText().setTextContents(operation)
    assert entry.queryAction().doAction(0)  # activate the location entry
    time.sleep(0.5)

names = ("Cancel",) if operation == "cancel" else ("OK", "Open", "Select")
button = next(
    (
        node
        for node in controls()
        if node.name in names
        and node.getRole() == pyatspi.ROLE_PUSH_BUTTON
        and visible(node)
    ),
    None,
)
if button is not None:
    assert button.queryAction().doAction(0)
else:
    # Activating a complete file path can accept and close the chooser itself.
    assert operation != "cancel"
    assert not any(
        node.getRole() == pyatspi.ROLE_FILE_CHOOSER and visible(node)
        for node in controls()
    )
Path(marker).write_text("PASS")
