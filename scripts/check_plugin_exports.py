#!/usr/bin/env python3
"""Verify the built Zellij WASM plugin exports the required entry points.

Usage: check_plugin_exports.py [WASM_PATH]

WASM_PATH defaults to the release build target path.
"""
import sys
from pathlib import Path

DEFAULT_WASM = "target/wasm32-wasip1/release/showy-quota-zellij.wasm"


def main(argv):
    wasm = Path(argv[1] if len(argv) > 1 else DEFAULT_WASM)
    data = wasm.read_bytes()
    if data[:8] != b"\0asm\x01\0\0\0":
        print("not a WebAssembly module", file=sys.stderr)
        sys.exit(1)

    def uleb(offset):
        value = 0
        shift = 0
        while True:
            byte = data[offset]
            offset += 1
            value |= (byte & 0x7F) << shift
            if byte < 0x80:
                return value, offset
            shift += 7

    exports = set()
    offset = 8
    while offset < len(data):
        section_id = data[offset]
        offset += 1
        section_size, offset = uleb(offset)
        section_end = offset + section_size
        if section_id == 7:
            count, offset = uleb(offset)
            for _ in range(count):
                name_len, offset = uleb(offset)
                name = data[offset:offset + name_len].decode()
                offset += name_len
                offset += 1
                _, offset = uleb(offset)
                exports.add(name)
        offset = section_end

    required = {"_start", "load", "update", "render", "plugin_version"}
    missing = required - exports
    if missing:
        print(f"missing WASM exports: {sorted(missing)}", file=sys.stderr)
        sys.exit(1)
    print(f"check plugin exports: ok ({wasm})")


if __name__ == "__main__":
    main(sys.argv)
