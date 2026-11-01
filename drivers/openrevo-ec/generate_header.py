#!/usr/bin/env python3
"""Generate validated kernel constants from the shared hardware registry."""
import argparse
import json
from pathlib import Path
import re
import uuid


def validate(registry):
    """Reject malformed values before emitting hardware constants or C code."""
    if not isinstance(registry, dict):
        raise ValueError('Hardware registry must be an object')
    groups = {'ec': 65535, 'mask': 255, 'linux_wmi': 4294967295,
              'linux_fan': 65535, 'linux_control': 65535}
    for group, maximum in groups.items():
        values = registry.get(group)
        if not isinstance(values, dict) or not values:
            raise ValueError(f'Missing or invalid {group} object')
        for name, value in values.items():
            if not re.fullmatch(r'[A-Z][A-Z0-9_]*', name):
                raise ValueError(f'Invalid C constant name: {group}.{name}')
            if group == 'linux_wmi' and name == 'GUID':
                if not isinstance(value, str):
                    raise ValueError('WMI GUID must be a string')
                uuid.UUID(value)
            elif type(value) is not int or not (1 if group == 'ec' else 0) <= value <= maximum:
                raise ValueError(f'Invalid unsigned integer: {group}.{name}')
    for group in ('linux_ec_read', 'linux_ec_write'):
        names = registry.get(group)
        if not isinstance(names, list) or any(not isinstance(name, str) for name in names):
            raise ValueError(f'Invalid register whitelist: {group}')
        if len(set(names)) != len(names) or any(name not in registry['ec'] for name in names):
            raise ValueError(f'Unknown or duplicate register in {group}')
    if not registry['linux_ec_read'] or registry['linux_ec_write']:
        raise ValueError('Semantic driver requires readable registers and no raw writes')
    wmi = registry['linux_wmi']
    if not isinstance(wmi.get('GUID'), str):
        raise ValueError('Missing WMI GUID')
    for name in ('INSTANCE', 'METHOD', 'READ', 'WRITE', 'ERROR_SENTINEL'):
        if name not in wmi:
            raise ValueError(f'Missing linux_wmi.{name}')
    if not 0 < wmi.get('FRAME_SIZE', 0) <= 256 or wmi.get('MIN_REPLY_SIZE', 0) < 4:
        raise ValueError('Invalid WMI frame size')
    occupied = set()
    for name, width in (('ADDRESS_OFFSET', 2), ('DATA_OFFSET', 1), ('FUNCTION_OFFSET', 1)):
        if name not in wmi or wmi[name] + width > wmi['FRAME_SIZE']:
            raise ValueError(f'Invalid WMI frame offset: {name}')
        span = set(range(wmi[name], wmi[name] + width))
        if span & occupied:
            raise ValueError(f'Overlapping WMI frame offset: {name}')
        occupied.update(span)
    if wmi['READ'] > 255 or wmi['WRITE'] > 255 or wmi['READ'] == wmi['WRITE']:
        raise ValueError('WMI operation codes must be distinct bytes')
    layouts = {
        'linux_fan': {'VERSION': 1, 'FRAME_SIZE': 100, 'VERSION_OFFSET': 0,
                      'ACTION_OFFSET': 1, 'TABLE_OFFSET': 2, 'TABLE_BYTES': 96,
                      'TABLE_STEPS': 16, 'RAMP_OFFSET': 98, 'ISOLATED_OFFSET': 99},
        'linux_control': {'VERSION': 1, 'FRAME_SIZE': 8, 'VERSION_OFFSET': 0,
                          'ACTION_OFFSET': 1, 'ARGUMENT_OFFSET': 2, 'RESERVED_OFFSET': 5},
    }
    for group, layout in layouts.items():
        for name, expected in layout.items():
            if registry[group].get(name) != expected:
                raise ValueError(f'Unsupported wire layout: {group}.{name}')
    required = {
        'linux_fan': ('REQUEST_OFFSET', 'APPLY', 'RESTORE', 'HEARTBEAT', 'DUTY_MAX',
                      'DUTY_MIN_ACTIVE', 'CONTROL_MIN', 'AP_ACTIVE_MASK', 'ISOLATED_BIT',
                      'RAMP_ENABLE', 'LEASE_MS', 'CUSTOM_OFF', 'CUSTOM_ON',
                      'OFFICE_CLAMP', 'BALANCED_CLAMP', 'TURBO_CLAMP'),
        'linux_control': ('REQUEST_OFFSET', 'MODE', 'POWER', 'BOOST', 'RECOVER',
                          'AP_PRESENT', 'MODE_PRESERVE', 'MODE_TURBO', 'RAMP_SMOOTH',
                          'MODE_STATUS_IDLE', 'MODE_STATUS_REQUEST',
                          'MIN_POWER', 'POWER_INVALID', 'PL4_MARGIN'),
    }
    for group, names in required.items():
        for name in names:
            if name not in registry[group]:
                raise ValueError(f'Missing {group}.{name}')
            if name not in ('REQUEST_OFFSET', 'LEASE_MS') and registry[group][name] > 255:
                raise ValueError(f'Protocol byte exceeds range: {group}.{name}')
    for group, actions in (('linux_fan', ('APPLY', 'RESTORE', 'HEARTBEAT')),
                           ('linux_control', ('MODE', 'POWER', 'BOOST', 'RECOVER'))):
        if len({registry[group][name] for name in actions}) != len(actions):
            raise ValueError(f'Duplicate protocol action in {group}')
    if registry['linux_control']['REQUEST_OFFSET'] == registry['linux_fan']['REQUEST_OFFSET']:
        raise ValueError('Control and fan request offsets must differ')
    for group in ('linux_fan', 'linux_control'):
        if registry[group]['REQUEST_OFFSET'] in registry['ec'].values():
            raise ValueError(f'{group} request offset overlaps a physical register')
    fan, control = registry['linux_fan'], registry['linux_control']
    if control['MODE_STATUS_IDLE'] != 0 or control['MODE_STATUS_REQUEST'] != 1:
        raise ValueError('Unsupported firmware mode status states')
    if not fan['LEASE_MS'] or fan['DUTY_MIN_ACTIVE'] > fan['DUTY_MAX'] or fan['CONTROL_MIN'] > fan['DUTY_MAX']:
        raise ValueError('Invalid fan lease or duty limits')
    if not control['MIN_POWER'] or not control['PL4_MARGIN'] or control['MIN_POWER'] + control['PL4_MARGIN'] >= control['POWER_INVALID']:
        raise ValueError('Invalid power bounds')


def generate(registry):
    validate(registry)
    lines = ['/* Generated from ec_registry.json. Do not edit. */']
    for group, prefix in [('ec', ''), ('mask', ''), ('linux_wmi', 'OR_WMI_'),
                          ('linux_fan', 'OR_FAN_'), ('linux_control', 'OR_CONTROL_')]:
        for name, value in registry[group].items():
            lines.append(f'#define {prefix}{name} {json.dumps(value)}')
    lines.append('static const u16 linux_ec_read[] = {')
    lines.extend(f'    {name},' for name in registry['linux_ec_read'])
    lines.append('};')
    return '\n'.join(lines) + '\n'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    root = Path(__file__).resolve().parents[2]
    parser.add_argument('--registry', type=Path, default=root / 'src-tauri/config/ec_registry.json')
    parser.add_argument('--output', type=Path, default=Path(__file__).with_name('vault_generated.h'))
    args = parser.parse_args()
    try:
        header = generate(json.loads(args.registry.read_text(encoding='utf-8')))
    except (OSError, ValueError, KeyError, TypeError) as error:
        parser.error(f'Invalid hardware registry: {error}')
    args.output.write_text(header, encoding='ascii')


if __name__ == '__main__':
    main()
