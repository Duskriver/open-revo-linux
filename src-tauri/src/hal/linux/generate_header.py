#!/usr/bin/env python3
"""Generate the kernel transport's constants from the shared hardware vault."""
import json
from pathlib import Path

root = Path(__file__).resolve().parents[3]
registry = json.loads((root / 'config/ec_registry.json').read_text())
lines = ['/* Generated from ec_registry.json. Do not edit. */']
for group, prefix in [('ec', ''), ('mask', ''), ('linux_wmi', 'OR_WMI_'), ('linux_fan', 'OR_FAN_')]:
    for name, value in registry[group].items():
        lines.append(f'#define {prefix}{name} {json.dumps(value)}')
for group in ['linux_ec_read', 'linux_ec_write']:
    lines.append(f'static const u16 {group}[] = {{')
    lines.extend(f'    {name},' for name in registry[group])
    lines.append('};')
Path(__file__).with_name('vault_generated.h').write_text('\n'.join(lines) + '\n')
