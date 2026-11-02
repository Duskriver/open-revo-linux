#!/usr/bin/env python3
"""Generate validated kernel constants from the shared hardware registry."""
import argparse
import json
from pathlib import Path
import re
import uuid


def validate(registry):
    if not isinstance(registry, dict):
        raise ValueError('Hardware registry must be an object')
    groups = {'ec': 65535, 'mask': 255, 'linux_wmi': 4294967295,
              'linux_fan': 65535, 'linux_transport': 255, 'linux_events': 65535,
              'tongfang_control': 255, 'linux_device': 255, 'wmi_hotkeys': 65535}
    for group, maximum in groups.items():
        values = registry.get(group)
        if not isinstance(values, dict) or not values:
            raise ValueError(f'Missing or invalid {group} object')
        for name, value in values.items():
            if not re.fullmatch(r'[A-Z][A-Z0-9_]*', name):
                raise ValueError(f'Invalid C constant name: {group}.{name}')
            if group == 'linux_wmi' and name in ('GUID', 'EVENT_GUID'):
                if not isinstance(value, str):
                    raise ValueError('WMI GUID must be a string')
                uuid.UUID(value)
            elif type(value) is not int or not (1 if group == 'ec' else 0) <= value <= maximum:
                raise ValueError(f'Invalid unsigned integer: {group}.{name}')
    for group in ('linux_ec_read', 'linux_ec_write'):
        names = registry.get(group)
        if not isinstance(names, list) or not names or any(not isinstance(name, str) for name in names):
            raise ValueError(f'Invalid register whitelist: {group}')
        if len(set(names)) != len(names) or any(name not in registry['ec'] for name in names):
            raise ValueError(f'Unknown or duplicate register in {group}')
        if len({registry['ec'][name] for name in names}) != len(names):
            raise ValueError(f'Duplicate physical register in {group}')
    if not set(registry['linux_ec_write']).issubset(registry['linux_ec_read']):
        raise ValueError('Writable registers must permit readback')
    wmi = registry['linux_wmi']
    for name in ('GUID', 'EVENT_GUID'):
        if not isinstance(wmi.get(name), str):
            raise ValueError(f'Missing WMI {name}')
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
        'linux_transport': {'VERSION': 1},
        'linux_device': {'VERSION': 1},
        'linux_events': {'VERSION': 1, 'FRAME_SIZE': 16, 'SEQUENCE_OFFSET': 0,
                         'CODE_OFFSET': 8, 'VERSION_OFFSET': 12, 'QUEUE_SIZE': 64},
        'linux_fan': {'VERSION': 2, 'FRAME_SIZE': 100, 'VERSION_OFFSET': 0,
                      'ACTION_OFFSET': 1, 'TABLE_OFFSET': 2, 'TABLE_BYTES': 96,
                      'TABLE_STEPS': 16, 'RAMP_OFFSET': 98, 'ISOLATED_OFFSET': 99},
    }
    for group, layout in layouts.items():
        for name, expected in layout.items():
            if registry[group].get(name) != expected:
                raise ValueError(f'Unsupported wire layout: {group}.{name}')
    fan = registry['linux_fan']
    for name in ('REQUEST_OFFSET', 'APPLY', 'RESTORE', 'HEARTBEAT', 'DUTY_MAX',
                 'DUTY_MIN_ACTIVE', 'CONTROL_MIN', 'AP_ACTIVE_MASK', 'ISOLATED_BIT',
                 'RAMP_ENABLE', 'LEASE_MS', 'CUSTOM_OFF', 'CUSTOM_ON',
                 'OFFICE_CLAMP', 'BALANCED_CLAMP', 'TURBO_CLAMP'):
        if name not in fan or (name not in ('REQUEST_OFFSET', 'LEASE_MS') and fan[name] > 255):
            raise ValueError(f'Invalid or missing linux_fan.{name}')
    if len({fan[name] for name in ('APPLY', 'RESTORE', 'HEARTBEAT')}) != 3:
        raise ValueError('Duplicate fan action')
    if fan['REQUEST_OFFSET'] in registry['ec'].values():
        raise ValueError('Fan request offset overlaps a physical register')
    if not fan['LEASE_MS'] or fan['DUTY_MIN_ACTIVE'] > fan['DUTY_MAX'] or fan['CONTROL_MIN'] > fan['DUTY_MAX']:
        raise ValueError('Invalid fan lease or duty limits')
    control = registry['tongfang_control']
    for name in ('AP_PRESENT', 'MODE_PRESERVE', 'MODE_TURBO', 'RAMP_SMOOTH',
                 'MIN_POWER', 'POWER_INVALID', 'PL4_MARGIN'):
        if name not in control:
            raise ValueError(f'Missing tongfang_control.{name}')
    if control.get('MODE_STATUS_IDLE') != 0 or control.get('MODE_STATUS_REQUEST') != 1:
        raise ValueError('Unsupported firmware mode status states')
    if not control['MIN_POWER'] or not control['PL4_MARGIN'] or control['MIN_POWER'] + control['PL4_MARGIN'] >= control['POWER_INVALID']:
        raise ValueError('Invalid power bounds')


def generate(registry):
    validate(registry)
    lines = ['/* Generated from ec_registry.json. Do not edit. */']
    for group, prefix in [('ec', ''), ('mask', ''), ('linux_wmi', 'OR_WMI_'),
                          ('linux_fan', 'OR_FAN_'), ('linux_transport', 'OR_TRANSPORT_'),
                          ('linux_events', 'OR_EVENT_'), ('linux_device', 'OR_DEVICE_')]:
        for name, value in registry[group].items():
            lines.append(f'#define {prefix}{name} {json.dumps(value)}')
    for group in ('linux_ec_read', 'linux_ec_write'):
        lines.append(f'static const u16 {group}[] = {{')
        lines.extend(f'    {name},' for name in registry[group])
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
