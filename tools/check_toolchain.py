#!/usr/bin/env python3
"""Describe or check development tools; never used by the package installer.

The window regression uses only Python's standard library. WebKitWebDriver must
come from the distribution's matching WebKitGTK package, not a separate pin.
"""
import argparse
import json
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]
EXACT_VERSION = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+")


def declared_tools():
    package = json.loads((ROOT / "package.json").read_text())
    rust = tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]
    tests = json.loads((ROOT / "tests/toolchain.json").read_text())
    manager = re.fullmatch(r"pnpm@([0-9]+\.[0-9]+\.[0-9]+)(?:\+sha[0-9]+\.[A-Za-z0-9+/=]+)?",
                           package.get("packageManager", ""))
    if manager is None:
        raise ValueError("packageManager must pin an exact pnpm version")
    versions = {
        "node": (ROOT / ".node-version").read_text().strip(),
        "rust": rust["channel"],
        "cargo": rust["channel"],
        "pnpm": manager[1],
        "python": tests["python"],
        "tauri_driver": tests["tauri-driver"],
    }
    for name, version in versions.items():
        if not isinstance(version, str) or not EXACT_VERSION.fullmatch(version):
            raise ValueError(f"{name} must use an exact major.minor.patch version")
    components = rust.get("components", [])
    if not isinstance(components, list) or "clippy" not in components:
        raise ValueError("rust-toolchain.toml must include the CI clippy component")
    if any(not isinstance(component, str) or not re.fullmatch(r"[a-z0-9-]+", component)
           for component in components):
        raise ValueError("Invalid Rust component name")
    for name in ("node", "pnpm"):
        engine = package.get("engines", {}).get(name)
        if engine is not None and engine != versions[name]:
            raise ValueError(f"engines.{name} differs from its pinned tool version")
    return versions, components


def command_version(command, pattern):
    result = subprocess.run(command, cwd=ROOT, check=True, text=True,
                            capture_output=True, timeout=30)
    match = re.search(pattern, result.stdout, re.MULTILINE)
    if match is None:
        raise ValueError(f"Cannot identify version from {' '.join(command)}")
    return match[1]


def installed_driver_version():
    executable = shutil.which("tauri-driver")
    if executable is None:
        raise ValueError("tauri-driver is not on PATH")
    # tauri-driver has no --version flag. Query the Cargo installation metadata
    # next to the actual executable so a different installation is not accepted.
    install_root = Path(executable).resolve().parent.parent
    return command_version(["cargo", "install", "--list", "--root", str(install_root)],
                           r"^tauri-driver v([0-9]+\.[0-9]+\.[0-9]+):$")


def check_tools(expected, with_driver):
    actual = {"python": platform.python_version()}
    errors = []
    commands = {
        "node": (["node", "--version"], r"^v([0-9]+\.[0-9]+\.[0-9]+)$"),
        "rust": (["rustc", "--version"], r"^rustc ([0-9]+\.[0-9]+\.[0-9]+) "),
        "cargo": (["cargo", "--version"], r"^cargo ([0-9]+\.[0-9]+\.[0-9]+) "),
        "pnpm": (["pnpm", "--version"], r"^([0-9]+\.[0-9]+\.[0-9]+)$"),
    }
    for name, (command, pattern) in commands.items():
        try:
            actual[name] = command_version(command, pattern)
        except (OSError, ValueError, subprocess.SubprocessError) as error:
            actual[name] = None
            errors.append(f"{name}: {error}")
    if with_driver:
        try:
            actual["tauri_driver"] = installed_driver_version()
        except (OSError, ValueError, subprocess.SubprocessError) as error:
            actual["tauri_driver"] = None
            errors.append(f"tauri_driver: {error}")
    for name, version in actual.items():
        if version is not None and version != expected[name]:
            message = f"{name}: expected {expected[name]}, found {version}"
            if name == "pnpm":
                message += "; run corepack enable pnpm, then verify pnpm --version"
            errors.append(message)
    return actual, errors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="Check installed development tool versions")
    parser.add_argument("--with-driver", action="store_true", help="Also check the installed tauri-driver")
    parser.add_argument("--github-output", type=Path, help="Append declared versions to a GitHub Actions output file")
    args = parser.parse_args()
    if args.with_driver and not args.check:
        parser.error("--with-driver requires --check")
    try:
        expected, components = declared_tools()
        metadata = {
            "schema": 1,
            "expected": expected,
            "sources": {
                "node": ".node-version",
                "rust": "rust-toolchain.toml",
                "pnpm": "package.json#packageManager",
                "test_tools": "tests/toolchain.json",
            },
            "rust_components": components,
            "python_dependencies": "standard library only",
            "webkit_driver": "distribution package matching WebKitGTK",
        }
        if args.github_output:
            with args.github_output.open("a", encoding="ascii") as output:
                for name, version in expected.items():
                    output.write(f"{name}={version}\n")
                output.write(f"rust_components={','.join(components)}\n")
        errors = []
        if args.check:
            metadata["actual"], errors = check_tools(expected, args.with_driver)
            metadata["errors"] = errors
        print(json.dumps(metadata, indent=2))
        for error in errors:
            print(error, file=sys.stderr)
        return bool(errors)
    except (KeyError, OSError, ValueError) as error:
        print(f"Toolchain configuration error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
