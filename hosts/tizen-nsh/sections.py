#!/usr/bin/env python3
"""Sizes of a WebAssembly module's sections: what the TV compiles (Code) and
what it only copies (Data). Compile time on a 2020 set follows the Code
section; data segments cost module and cache bytes.

    sections.py MODULE [--json] [--max-code BYTES]

With --max-code it fails when the Code section is larger (the CI budget).
No dependencies: it reads the section headers itself.
"""
import argparse
import json
import sys

NAMES = {0: "custom", 1: "type", 2: "import", 3: "function", 4: "table", 5: "memory",
         6: "global", 7: "export", 8: "start", 9: "element", 10: "code", 11: "data", 12: "datacount"}


def leb(data, at):
    value = shift = 0
    while True:
        byte = data[at]
        at += 1
        value |= (byte & 0x7F) << shift
        shift += 7
        if not byte & 0x80:
            return value, at


def sections(data):
    if data[:4] != b"\0asm":
        raise ValueError("not a WebAssembly module")
    at, sizes = 8, {}
    while at < len(data):
        kind = data[at]
        size, body = leb(data, at + 1)
        name = NAMES.get(kind, f"unknown-{kind}")
        if kind == 0:
            length, start = leb(data, body)
            name = "custom:" + data[start:start + length].decode("utf-8", "replace")
        sizes[name] = sizes.get(name, 0) + size
        at = body + size
    return sizes


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("module")
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--max-code", type=int)
    options = parser.parse_args(argv)
    data = open(options.module, "rb").read()
    sizes = sections(data)
    report = {"total": len(data), "code": sizes.get("code", 0), "data": sizes.get("data", 0), "sections": sizes}
    if options.json:
        print(json.dumps(report, indent=2))
    else:
        print(f"total {report['total']:>10,} bytes")
        print(f"code  {report['code']:>10,} bytes")
        print(f"data  {report['data']:>10,} bytes")
    if options.max_code is not None and report["code"] > options.max_code:
        print(f"the Code section ({report['code']:,} bytes) is over its budget of {options.max_code:,}",
              file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
