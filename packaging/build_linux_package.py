#!/usr/bin/env python3
"""Build or verify the complete OpenRevo Debian package, without installing it."""
import argparse
import hashlib
import importlib.util
import io
import os
from pathlib import Path, PurePosixPath
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('driver_package', ROOT / 'packaging/build_driver_package.py')
DRIVER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(DRIVER)
TEMPLATES = ROOT / 'packaging/debian'
PACKAGE = 'open-revo'
DRIVER_DEPENDENCIES = (
    'dkms (>= 2.8.7)', 'python3', 'kmod', 'udev', 'systemd', 'acl',
    'linux-headers-generic | linux-headers-amd64 | linux-headers',
)
APP_DEPENDENCIES = ('libwebkit2gtk-4.1-0', 'libayatana-appindicator3-1')
CONFFILES = ('/etc/modprobe.d/openrevo-ec.conf', '/etc/modules-load.d/openrevo-ec.conf')


def fields(text):
    """Read one Debian control stanza while rejecting ambiguous metadata."""
    result = {}
    key = None
    for line in text.splitlines():
        if line.startswith((' ', '\t')) and key:
            result[key] += '\n' + line
        elif ':' in line:
            key, value = line.split(':', 1)
            if key in result:
                raise ValueError(f'Duplicate control field: {key}')
            result[key] = value.strip()
        elif line:
            raise ValueError('Malformed Debian control stanza')
    return result


def archive_files(package, control=False):
    """Inspect before extraction; our packages need only regular files/directories."""
    switch = '--ctrl-tarfile' if control else '--fsys-tarfile'
    data = subprocess.check_output(['dpkg-deb', switch, str(package)])
    result = {}
    with tarfile.open(fileobj=io.BytesIO(data)) as archive:
        for member in archive:
            name = member.name.removeprefix('./')
            path = PurePosixPath(name)
            if path.is_absolute() or '..' in path.parts:
                raise ValueError(f'Unsafe package path: {member.name}')
            if member.isdir():
                continue
            if not member.isfile() or name in result:
                raise ValueError(f'Unsupported or duplicate package member: {member.name}')
            if member.uid != 0 or member.gid != 0:
                raise ValueError(f'Non-root package ownership: {name}')
            result[name] = (archive.extractfile(member).read(), member.mode)
    return result


def metadata(package):
    return fields(subprocess.check_output(['dpkg-deb', '--field', str(package)], text=True))


def check_identity(control, version):
    for key, value in (('Package', PACKAGE), ('Version', version), ('Architecture', 'amd64')):
        if control.get(key) != value:
            raise ValueError(f'Expected {key}: {value}; got {control.get(key)!r}')


def render_template(name, version):
    return (TEMPLATES / name).read_text(encoding='ascii').replace('@VERSION@', version)


def verify(package, version=None):
    version = DRIVER.version_value(version or DRIVER.default_version())
    control = metadata(package)
    check_identity(control, version)
    dependencies = {part.strip() for part in control.get('Depends', '').split(',')}
    for dependency in APP_DEPENDENCIES + DRIVER_DEPENDENCIES:
        if dependency not in dependencies:
            raise ValueError(f'Missing dependency: {dependency}')
    for key in ('Conflicts', 'Replaces'):
        if control.get(key) != DRIVER.PACKAGE:
            raise ValueError(f'Missing split-package migration field: {key}')
    if control.get('Provides') != f'{DRIVER.PACKAGE} (= {version})':
        raise ValueError('Missing versioned driver package provision')
    payload = archive_files(package)
    scripts = archive_files(package, control=True)
    required = {
        'usr/bin/open-revo', 'usr/share/applications/OpenRevo.desktop',
        'usr/lib/open-revo/driver-setup', 'usr/lib/udev/rules.d/70-openrevo-ec.rules',
        'usr/share/doc/open-revo/copyright', 'usr/share/doc/open-revo/README.Debian',
        'usr/share/doc/open-revo/build-info.json',
        *(path.lstrip('/') for path in CONFFILES),
    }
    prefix = f'usr/src/openrevo-ec-{version}/'
    required.update(prefix + name for name in DRIVER.SOURCES + (
        'vault_generated.h', 'Makefile', 'dkms.conf', 'config/ec_registry.json', 'LICENSE'))
    missing = required - payload.keys()
    if missing:
        raise ValueError(f'Missing complete-package payload: {sorted(missing)}')
    if any(name.endswith(('.ko', '.ko.xz', '.ko.zst', '.o', '.exe')) for name in payload):
        raise ValueError('Unexpected precompiled driver or Windows payload')
    if {name for name in payload if name.startswith('etc/')} != {path.lstrip('/') for path in CONFFILES}:
        raise ValueError('Unexpected system configuration files')
    if any(name.startswith('usr/src/') and not name.startswith(prefix) for name in payload):
        raise ValueError('Unexpected driver source version')
    if scripts.get('conffiles', (b'', 0))[0].decode().splitlines() != list(CONFFILES):
        raise ValueError('Driver configuration must be registered as conffiles')
    for name in ('postinst', 'prerm', 'postrm'):
        content, mode = scripts.get(name, (b'', 0))
        if content != render_template(name + '.in', version).encode('ascii') or mode != 0o755:
            raise ValueError(f'Incorrect lifecycle script: {name}')
    helper, mode = payload['usr/lib/open-revo/driver-setup']
    if helper != (TEMPLATES / 'driver-setup').read_bytes() or mode != 0o755:
        raise ValueError('Incorrect driver setup helper')
    expected_hashes = ''.join(
        f'{hashlib.md5(content).hexdigest()}  {name}\n'
        for name, (content, _) in sorted(payload.items())
    ).encode('ascii')
    if scripts.get('md5sums', (b'', 0))[0] != expected_hashes:
        raise ValueError('Package payload checksums do not match')
    # Match packaged sources to this checkout, including the hardware dictionary.
    for name in DRIVER.SOURCES:
        if payload[prefix + name][0] != (ROOT / 'drivers/openrevo-ec' / name).read_bytes():
            raise ValueError(f'Packaged driver source differs from checkout: {name}')
    if payload[prefix + 'config/ec_registry.json'][0] != (ROOT / 'src-tauri/config/ec_registry.json').read_bytes():
        raise ValueError('Packaged hardware dictionary differs from checkout')
    print(f'Verified complete package: {package}')
    return control


def build(app_package, output_dir):
    version = DRIVER.version_value(DRIVER.default_version())
    app_package = Path(app_package).resolve(strict=True)
    output = Path(output_dir).resolve()
    release_root = (ROOT / 'dist_release').resolve()
    if release_root == ROOT or not release_root.is_relative_to(ROOT) or not output.is_relative_to(release_root):
        raise ValueError('Output directory must be inside this project\'s dist_release directory')
    control = metadata(app_package)
    check_identity(control, version)
    payload = archive_files(app_package)
    original_scripts = archive_files(app_package, control=True)
    if not {'usr/bin/open-revo', 'usr/share/applications/OpenRevo.desktop'} <= payload.keys():
        raise ValueError('Input package is missing the desktop application')
    if any(name.startswith(('etc/', 'usr/src/', 'usr/lib/open-revo/')) for name in payload):
        raise ValueError('Input must be a Tauri application-only package')
    if original_scripts.keys() - {'control', 'md5sums'}:
        raise ValueError('Unexpected Tauri lifecycle metadata; review before merging')
    output.mkdir(parents=True, exist_ok=True)
    stage = Path(tempfile.mkdtemp(prefix='linux-package-', dir=output))
    package_root = stage / 'root'
    subprocess.run(['dpkg-deb', '--raw-extract', str(app_package), str(package_root)], check=True)
    DRIVER.stage_payload(version, ROOT / 'drivers/openrevo-ec', package_root, PACKAGE)
    DRIVER.write(package_root / 'usr/lib/open-revo/driver-setup',
                 (TEMPLATES / 'driver-setup').read_text(encoding='ascii'), executable=True)
    DRIVER.write(package_root / 'usr/share/doc/open-revo/README.Debian',
                 render_template('README.Debian', version))
    dependencies = [part.strip() for part in control.get('Depends', '').split(',') if part.strip()]
    control.update({
        'Depends': ', '.join(dict.fromkeys(dependencies + list(APP_DEPENDENCIES + DRIVER_DEPENDENCIES))),
        'Conflicts': DRIVER.PACKAGE,
        'Replaces': DRIVER.PACKAGE,
        'Provides': f'{DRIVER.PACKAGE} (= {version})',
        'Section': 'utils',
        'Homepage': 'https://github.com/Duskriver/open-revo-linux',
        'Description': 'OpenRevo Linux laptop control center with EC driver\n'
                       ' Desktop application and DKMS source for supported Mechrevo laptops.\n'
                       ' Installs the driver using distribution kernel headers and DKMS signing.',
    })
    files = sorted(path for path in package_root.rglob('*') if path.is_file()
                   and not path.is_relative_to(package_root / 'DEBIAN'))
    control['Installed-Size'] = str(sum((path.stat().st_size + 1023) // 1024 for path in files))
    DRIVER.write(package_root / 'DEBIAN/control', ''.join(f'{key}: {value}\n' for key, value in control.items()))
    DRIVER.write(package_root / 'DEBIAN/conffiles', '\n'.join(CONFFILES) + '\n')
    for name in ('postinst', 'prerm', 'postrm'):
        script = package_root / 'DEBIAN' / name
        DRIVER.write(script, render_template(name + '.in', version), executable=True)
        subprocess.run(['sh', '-n', str(script)], check=True)
    DRIVER.write(package_root / 'DEBIAN/md5sums', ''.join(
        f'{hashlib.md5(path.read_bytes()).hexdigest()}  {path.relative_to(package_root).as_posix()}\n'
        for path in files))
    package = output / f'OpenRevo_{version}_amd64.deb'
    staged_package = stage / package.name
    subprocess.run(['dpkg-deb', '--root-owner-group', '--build', str(package_root), str(staged_package)], check=True)
    verify(staged_package, version)
    os.replace(staged_package, package)
    print(f'Install with: sudo apt install {package}')
    print(f'Preserved build stage: {stage}')
    return package, stage


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--app-package', type=Path)
    parser.add_argument('--output-dir', type=Path, default=ROOT / 'dist_release')
    parser.add_argument('--verify', type=Path, metavar='PACKAGE')
    args = parser.parse_args()
    try:
        if args.verify:
            verify(args.verify)
        else:
            source = args.app_package or ROOT / 'src-tauri/target/release/bundle/deb' / f'OpenRevo_{DRIVER.default_version()}_amd64.deb'
            build(source, args.output_dir)
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError, tarfile.TarError) as error:
        parser.exit(1, f'Complete package operation failed: {error}\n')


if __name__ == '__main__':
    main()
