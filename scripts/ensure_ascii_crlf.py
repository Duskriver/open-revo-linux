#!/usr/bin/env python3
"""Validate ASCII maintenance scripts and normalize their Windows line endings."""
from pathlib import Path
import sys


def normalize(path):
    if path.suffix.lower() not in {".ps1", ".bat"}:
        raise ValueError(f"Expected a .ps1 or .bat file: {path}")
    data = path.read_bytes()
    if any(byte not in (9, 10, 13) and not 32 <= byte <= 126 for byte in data):
        raise ValueError(f"Non-ASCII or unsupported control character: {path}")
    clean = data.replace(b"\r\n", b"\n").replace(b"\r", b"\n").replace(b"\n", b"\r\n")
    if clean != data:
        path.write_bytes(clean)
    assert b"\n" not in path.read_bytes().replace(b"\r\n", b"")
    print(f"PASS: ASCII and CRLF: {path}")


if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit("Usage: python scripts/ensure_ascii_crlf.py PATH [PATH ...]")
    for argument in sys.argv[1:]:
        normalize(Path(argument).resolve(strict=True))
