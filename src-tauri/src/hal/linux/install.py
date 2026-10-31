#!/usr/bin/env python3
"""Install an already-built transport for the running kernel. No GUI as root."""
import os
from pathlib import Path
import platform
import shutil
import subprocess

source = Path(__file__).resolve().parent
if os.geteuid() != 0:
    raise SystemExit('Run with sudo after building: make -C src-tauri/src/hal/linux')
module = source / 'openrevo_ec.ko'
inputs = [source / 'openrevo_ec.c', source / 'generate_header.py',
          source / 'fan_transaction.h', source / 'Makefile', source.parents[2] / 'config/ec_registry.json']
if not module.is_file() or any(p.stat().st_mtime_ns > module.stat().st_mtime_ns for p in inputs):
    raise SystemExit('Missing or stale module. Build successfully before installing.')
if Path('/sys/module/openrevo_ec').exists():
    raise SystemExit('Close OpenRevo, run sudo modprobe -r openrevo_ec, then reinstall.')
kernel = platform.release()
vermagic = subprocess.check_output(['modinfo', '-F', 'vermagic', str(module)], text=True)
if vermagic.split()[0] != kernel:
    raise SystemExit('Module does not match the running kernel; rebuild it first.')
target = Path('/lib/modules') / kernel / 'extra/openrevo'
target.mkdir(parents=True, exist_ok=True)
shutil.copy2(module, target / module.name)
shutil.copy2(source / '70-openrevo-ec.rules', '/etc/udev/rules.d/70-openrevo-ec.rules')
Path('/etc/modules-load.d/openrevo-ec.conf').write_text('openrevo_ec\n')
Path('/etc/modprobe.d/openrevo-ec.conf').write_text('options openrevo_ec allow_writes=1\n')
subprocess.run(['depmod', '-a', kernel], check=True)
subprocess.run(['udevadm', 'control', '--reload-rules'], check=True)
subprocess.run(['modprobe', 'openrevo_ec'], check=True)
subprocess.run(['udevadm', 'trigger', '--action=change', '/sys/class/misc/openrevo_ec'], check=True)
subprocess.run(['udevadm', 'settle'], check=True)
print('Installed for ' + kernel + '. Rebuild/reinstall after a kernel upgrade.')
print('Only the validated DMI/BIOS can bind. Secure Boot requires module signing.')
