#!/usr/bin/env python3
"""Build the independent DKMS Debian package without installing any software."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
TEMPLATES = ROOT / 'packaging/dkms'
PACKAGE = 'openrevo-ec-dkms'
SOURCES = (
    'openrevo_ec.c', 'fan_transaction.h', 'control_transaction.h',
    'test_fan_transaction.c', 'test_control_transaction.c', 'generate_header.py',
    'test_generate_header.py',
)


def version_value(value):
    if not re.fullmatch(r'[0-9][0-9A-Za-z.+~-]*', value):
        raise ValueError('Version must start with a digit and use Debian version characters only')
    return value


def default_version():
    return json.loads((ROOT / 'src-tauri/tauri.conf.json').read_text())['version']


def write(path, content, executable=False):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding='ascii')
    path.chmod(0o755 if executable else 0o644)


def template(name, version):
    return (TEMPLATES / name).read_text(encoding='ascii').replace('@VERSION@', version)


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def build(version, driver_source, output_dir):
    version = version_value(version)
    source = Path(driver_source).resolve(strict=True)
    output = Path(output_dir).resolve()
    release_root = (ROOT / 'dist_release').resolve()
    if release_root == ROOT or not release_root.is_relative_to(ROOT) or not output.is_relative_to(release_root):
        raise ValueError('Output directory must be inside this project\'s dist_release directory')
    inputs = {}
    for name in SOURCES + ('70-openrevo-ec.rules',):
        path = source / name
        if not path.is_file() or path.is_symlink():
            raise ValueError(f'Missing regular driver source: {path}')
        inputs[path] = sha256(path)
    registry = ROOT / 'src-tauri/config/ec_registry.json'
    inputs[registry] = sha256(registry)
    output.mkdir(parents=True, exist_ok=True)
    stage = Path(tempfile.mkdtemp(prefix='driver-package-', dir=output))
    package_root = stage / 'root'
    module = package_root / 'usr/src' / f'openrevo-ec-{version}'
    module.mkdir(parents=True)
    for name in SOURCES:
        shutil.copyfile(source / name, module / name)
        (module / name).chmod(0o644)
    (module / 'config').mkdir()
    shutil.copyfile(registry, module / 'config/ec_registry.json')
    subprocess.run([
        sys.executable, str(module / 'generate_header.py'),
        '--registry', str(module / 'config/ec_registry.json'),
        '--output', str(module / 'vault_generated.h'),
    ], check=True)
    write(module / 'Makefile', template('Makefile', version))
    write(module / 'dkms.conf', template('dkms.conf.in', version))
    shutil.copyfile(ROOT / 'LICENSE', module / 'LICENSE')
    doc = package_root / 'usr/share/doc' / PACKAGE
    doc.mkdir(parents=True)
    shutil.copyfile(ROOT / 'LICENSE', doc / 'copyright')
    write(doc / 'README.Debian', template('README.Debian', version))
    write(doc / 'build-info.json', json.dumps({
        'version': version,
        'source_sha256': {path.name: digest for path, digest in inputs.items()},
    }, indent=2) + '\n')
    rules = package_root / 'usr/lib/udev/rules.d/70-openrevo-ec.rules'
    rules.parent.mkdir(parents=True)
    shutil.copyfile(source / '70-openrevo-ec.rules', rules)
    write(package_root / 'etc/modules-load.d/openrevo-ec.conf', 'openrevo_ec\n')
    write(package_root / 'etc/modprobe.d/openrevo-ec.conf', 'options openrevo_ec allow_writes=1\n')
    control = package_root / 'DEBIAN'
    size = sum(path.stat().st_size for path in package_root.rglob('*') if path.is_file())
    write(control / 'control', f'''Package: {PACKAGE}
Version: {version}
Section: kernel
Priority: optional
Architecture: all
Maintainer: OpenRevo Linux contributors <noreply@openrevo.invalid>
Depends: dkms (>= 2.8.7), python3, kmod, udev
Recommends: linux-headers-generic | linux-headers-amd64
Installed-Size: {(size + 1023) // 1024}
Homepage: https://github.com/Duskriver/open-revo-linux
Description: DKMS source for the model-verified OpenRevo EC driver
 Provides the Linux WMI control module for supported Mechrevo laptops.
 Requires matching kernel headers. Uses distribution DKMS signing settings.
''')
    write(control / 'conffiles', '/etc/modprobe.d/openrevo-ec.conf\n/etc/modules-load.d/openrevo-ec.conf\n')
    for script in ('postinst', 'prerm', 'postrm'):
        write(control / script, template(script + '.in', version), executable=True)
        subprocess.run(['sh', '-n', str(control / script)], check=True)
    for path, digest in inputs.items():
        if sha256(path) != digest:
            raise RuntimeError(f'Source changed during packaging: {path}; rebuild after edits finish')
    package = output / f'{PACKAGE}_{version}_all.deb'
    staged_package = stage / package.name
    subprocess.run(['dpkg-deb', '--root-owner-group', '--build', str(package_root), str(staged_package)], check=True)
    os.replace(staged_package, package)
    print(f'Package: {package}')
    print(f'Preserved build stage: {stage}')
    return package, stage


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--version', default=default_version())
    parser.add_argument('--driver-source', type=Path, default=ROOT / 'drivers/openrevo-ec')
    parser.add_argument('--output-dir', type=Path, default=ROOT / 'dist_release')
    args = parser.parse_args()
    try:
        build(args.version, args.driver_source, args.output_dir)
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        parser.exit(1, f'Driver package build failed: {error}\n')


if __name__ == '__main__':
    main()
