#!/usr/bin/env python3
"""Static EC address gate for business code; not a substitute for HAL review."""
import json
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[1]
SOURCE_ROOTS = ("src", "src-tauri/src", "src-tauri/crates/openrevo-core/src")
HAL_ROOTS = ("src-tauri/src/hal/", "src-tauri/crates/openrevo-core/src/hal/")
NUMBER = re.compile(
    r"(?<![\w.])(?:0[xX][0-9a-fA-F_]+|[0-9][0-9_]*)(?:u(?:8|16|32|64|size)|i(?:8|16|32|64|size))?(?![\w.])"
)
IDENTIFIER = re.compile(r"\b[A-Za-z_]\w*\b")
# Only the captured numeric token is exempted, never the containing line.
# These values coincide with registered addresses but have non-hardware units.
NON_ADDRESS_VALUES = tuple(re.compile(pattern) for pattern in (
    r"from_millis\((2000)\)",
    r"setTimeout\([^;\n]*,\s*(2000)\)",
    r"fetch_single_manifest\([^,\n]+,\s*(2000)\)",
    r"\.Next\((2000),",
    r"WaitForSingleObject\([^,\n]+,\s*(2000)\)",
    r"now_ms\.saturating_sub\(last_cmd_ms\)\s*>\s*(2000)\b",
    r"\.clamp\((2000),\s*12000\)",
    r"ebx\s*>=\s*(2000)\s*&&\s*ebx\s*<=\s*7000",
    r"width\s*>=\s*(3840)\b",
    r"http://schemas\.microsoft\.com/windows/(2004)/02/mit/task",
    r"gpu_vram_mb:\s*(8192)\b",
    r"let\s+(?:vram_mb|final_vram)\s*=.*?else\s*\{\s*(8192)\s*\}",
    r"let\s+vram\s*=.*?\.unwrap_or\((8192)\)",
    r"vram_mb:\s*AtomicU64::new\((8192)\)",
))
RAW_EC_CALL = re.compile(
    r"\b(?:\w*read_ec|\w*write_ec|\w*modify_ec)\s*\(\s*(?:&\s*)?"
    r"(0[xX][0-9a-fA-F_]+|[0-9][0-9_]*)\b"
)
RAW_CONSTANT = re.compile(
    r"\b(?:const|let|static)\s+(?:ADDR_|IOCTL_|PORT_)[A-Z0-9_]+\s*"
    r"(?::[^=\n]+)?=\s*(0[xX][0-9a-fA-F_]+|[0-9][0-9_]*)\b"
)


def scan_text(text, registry):
    addresses = set(registry["ec"].values())
    forbidden = addresses | {
        value for key, value in registry["driver"].items() if key.startswith("IOCTL_")
    }
    # Existing generated aliases are valid vault references, including legacy names.
    aliases = {key for group in registry.values() if isinstance(group, dict) for key in group}
    findings = []
    for line_number, line in enumerate(text.splitlines(), 1):
        exempt = {match.span(1) for pattern in NON_ADDRESS_VALUES for match in pattern.finditer(line)}
        for match in NUMBER.finditer(line):
            token = re.sub(r"[ui](?:8|16|32|64|size)$", "", match.group())
            value = int(token.replace("_", ""), 16 if token.lower().startswith("0x") else 10)
            if value in forbidden and match.span() not in exempt:
                findings.append((line_number, f"raw registered address/IOCTL: {match.group()}"))
        for match in IDENTIFIER.finditer(line):
            name = match.group()
            if name not in aliases and any(int(part) in addresses for part in re.findall(r"\d+", name)):
                findings.append((line_number, f"address-based identifier: {name}"))
        for pattern in (RAW_EC_CALL, RAW_CONSTANT):
            for match in pattern.finditer(line):
                findings.append((line_number, f"literal hardware access/constant: {match.group().strip()}"))
    return sorted(set(findings))


def main():
    registry = json.loads((ROOT / "src-tauri/config/ec_registry.json").read_text(encoding="utf-8"))
    count = 0
    failures = 0
    for source_root in SOURCE_ROOTS:
        for path in sorted((ROOT / source_root).rglob("*")):
            if path.suffix not in {".rs", ".ts", ".tsx", ".js", ".jsx"}:
                continue
            relative = path.relative_to(ROOT)
            if relative.as_posix().startswith(HAL_ROOTS):
                continue
            count += 1
            for line, reason in scan_text(path.read_text(encoding="utf-8"), registry):
                print(f"{relative.as_posix()}:{line}: {reason}")
                failures += 1
    if failures:
        print(f"FAIL: {failures} vault boundary violations in {count} business source files.")
        return 1
    print(f"PASS: scanned {count} business source files; no vault boundary violations.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
