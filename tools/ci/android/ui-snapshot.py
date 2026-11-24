#!/usr/bin/env python3
"""Read synthetic smoke UI state from Android's native accessibility snapshot."""
import argparse
from pathlib import Path
import re
import sys
import xml.etree.ElementTree as ET

CONFIRMATION = "Remove all favorites in this list?"


def labels(node):
    return {value for key in ("text", "content-desc") if (value := node.get(key))}


def selected(root, label):
    return any(node.get("selected") == "true" and label in labels(node)
               for node in root.iter("node"))


def selected_label(root):
    names = {node.get("content-desc") or node.get("text")
             for node in root.iter("node") if node.get("selected") == "true"}
    names.discard(None)
    names.discard("")
    if len(names) != 1:
        raise ValueError("expected one selected accessibility label")
    return names.pop()


def favorites_count(root):
    counts = {tuple(map(int, match.groups()))
              for node in root.iter("node") for label in labels(node)
              if (match := re.fullmatch(r"Favorites · .+ · (\d+) of (\d+)", label))}
    if len(counts) != 1:
        raise ValueError("expected one visible favorites count")
    count, total = counts.pop()
    if count > total:
        raise ValueError("invalid favorites count")
    return count, total


def confirmation_open(root):
    return any(CONFIRMATION in labels(node) for node in root.iter("node"))


def editor_closed(root):
    return (next(root.iter("node"), None) is not None and not confirmation_open(root)
            and not any(label.startswith("Favorites · ")
                        for node in root.iter("node") for label in labels(node)))


def button_center(root, label):
    centers = set()
    for node in root.iter("node"):
        if label not in labels(node) or node.get("clickable") != "true" or node.get("enabled") != "true":
            continue
        match = re.fullmatch(r"\[(\d+),(\d+)\]\[(\d+),(\d+)\]", node.get("bounds", ""))
        if match:
            left, top, right, bottom = map(int, match.groups())
            if right > left and bottom > top:
                centers.add(((left + right) // 2, (top + bottom) // 2))
    if len(centers) != 1:
        raise ValueError("expected one visible enabled button")
    return centers.pop()


def covered_button_center(before, modal, label):
    """Reuse an underlying button's measured point without hitting dialog actions."""
    if not confirmation_open(modal):
        raise ValueError("expected an open confirmation over the measured button")
    x, y = button_center(before, label)
    actions = set()
    for node in modal.iter("node"):
        names = labels(node).intersection({"Cancel", "Remove all"})
        if not names or node.get("clickable") != "true":
            continue
        actions.update(names)
        match = re.fullmatch(r"\[(\d+),(\d+)\]\[(\d+),(\d+)\]", node.get("bounds", ""))
        if not match:
            raise ValueError("cannot locate confirmation button bounds")
        left, top, right, bottom = map(int, match.groups())
        if left <= x < right and top <= y < bottom:
            raise ValueError("underlying touch would hit a confirmation button")
    if actions != {"Cancel", "Remove all"}:
        raise ValueError("expected both confirmation actions")
    return x, y


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("snapshot", type=Path)
    parser.add_argument("action", choices=("selected", "selected-label", "count", "confirmation", "editor-closed", "tap", "covered-tap"))
    parser.add_argument("label", nargs="?")
    parser.add_argument("--before", type=Path, help="snapshot before opening the confirmation")
    args = parser.parse_args()
    try:
        root = ET.parse(args.snapshot).getroot()
        if args.action == "selected":
            if not args.label:
                parser.error("selected requires a label")
            return 0 if selected(root, args.label) else 1
        if args.action == "selected-label":
            print(selected_label(root))
        elif args.action == "count":
            print(*favorites_count(root))
        elif args.action == "confirmation":
            return 0 if confirmation_open(root) else 1
        elif args.action == "editor-closed":
            return 0 if editor_closed(root) else 1
        else:
            if not args.label:
                parser.error("tap requires a label")
            if args.action == "covered-tap":
                if args.before is None:
                    parser.error("covered-tap requires --before")
                print(*covered_button_center(ET.parse(args.before).getroot(), root, args.label))
            else:
                print(*button_center(root, args.label))
    except (ValueError, ET.ParseError, OSError) as error:
        print(f"native UI snapshot: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
