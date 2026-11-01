"""Hardware-free tests for the DKMS package and Debian lifecycle scripts."""
import importlib.util
import io
import os
from pathlib import Path
import socket
import subprocess
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('driver_package', ROOT / 'packaging/build_driver_package.py')
PACKAGE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PACKAGE)


class DriverPackageTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        # Preserve fixtures in the project instead of recursively deleting them.
        release = ROOT / 'dist_release'
        release.mkdir(exist_ok=True)
        cls.output = Path(tempfile.mkdtemp(prefix='driver-package-tests-', dir=release))
        driver = ROOT / 'drivers/openrevo-ec'
        cls.version = PACKAGE.default_version()
        cls.package, cls.stage = PACKAGE.build(cls.version, driver, cls.output)
        cls.package_root = cls.stage / 'root'

    def lifecycle(self, script, action, previous='', *, status='', helper_rc=0,
                  status_rc=0, remove_rc=0, missing_helper=False, manual=False):
        case = Path(tempfile.mkdtemp(prefix='lifecycle-', dir=self.output))
        binary = case / 'bin'
        binary.mkdir()
        log = case / 'calls'
        helper = binary / 'common.postinst'
        log_function = '''record() {
    printf '%s' "$0" >> "$TEST_LOG"
    for argument do printf '|%s' "$argument" >> "$TEST_LOG"; done
    printf '\\n' >> "$TEST_LOG"
}
record "$@"
'''
        if not missing_helper:
            PACKAGE.write(helper, '#!/bin/sh\n' + log_function + 'exit "$HELPER_RC"\n', True)
        PACKAGE.write(binary / 'dkms', '#!/bin/sh\n' + log_function + '''case "$1" in
    status) printf '%s' "$DKMS_STATUS"; exit "$STATUS_RC" ;;
    remove) exit "$REMOVE_RC" ;;
    *) exit 90 ;;
esac
''', True)
        PACKAGE.write(binary / 'udevadm', '#!/bin/sh\n' + log_function + 'exit 0\n', True)
        for forbidden in ('modprobe', 'rmmod', 'rm'):
            PACKAGE.write(binary / forbidden, '#!/bin/sh\n' + log_function + 'exit 99\n', True)
        modules = case / 'modules'
        modules.mkdir()
        if manual:
            legacy = modules / 'test-kernel/extra/openrevo/openrevo_ec.ko.zst'
            legacy.parent.mkdir(parents=True)
            legacy.write_bytes(b'preserve the manually installed module')
        content = (self.package_root / 'DEBIAN' / script).read_text()
        # Substitute dependency locations only; exercise the actual packaged script.
        content = content.replace('/usr/lib/dkms/common.postinst', str(helper))
        content = content.replace('/lib/modules', str(modules))
        content = content.replace('/run/udev/control', str(case / 'udev-control'))
        executable = case / script
        # Fixture paths can contain non-ASCII workspace names; shipped scripts do not.
        executable.write_text(content, encoding='utf-8')
        executable.chmod(0o755)
        environment = dict(os.environ, PATH=str(binary), TEST_LOG=str(log),
                           DKMS_STATUS=status, HELPER_RC=str(helper_rc),
                           STATUS_RC=str(status_rc), REMOVE_RC=str(remove_rc))
        with socket.socket(socket.AF_UNIX) as control:
            # Bind by a short relative name; workspace paths can exceed UNIX limits.
            previous_directory = Path.cwd()
            try:
                os.chdir(case)
                control.bind('udev-control')
            finally:
                os.chdir(previous_directory)
            result = subprocess.run(['/bin/sh', str(executable), action, previous],
                                    env=environment, text=True, capture_output=True)
        return result, log.read_text() if log.exists() else '', case

    def test_package_contains_complete_source_and_only_owned_configuration(self):
        data = subprocess.check_output(['dpkg-deb', '--fsys-tarfile', str(self.package)])
        with tarfile.open(fileobj=io.BytesIO(data)) as archive:
            members = {item.name.removeprefix('./'): item for item in archive if item.isfile()}
            prefix = f'usr/src/openrevo-ec-{self.version}/'
            for filename in PACKAGE.SOURCES + ('vault_generated.h', 'Makefile', 'dkms.conf',
                                               'config/ec_registry.json', 'LICENSE'):
                self.assertIn(prefix + filename, members)
            self.assertIn('usr/lib/udev/rules.d/70-openrevo-ec.rules', members)
            self.assertEqual({name for name in members if name.startswith('etc/')}, {
                'etc/modprobe.d/openrevo-ec.conf', 'etc/modules-load.d/openrevo-ec.conf'})
            self.assertTrue(all(member.uid == member.gid == 0 for member in members.values()))
            self.assertFalse(any(name.endswith(('.ko', '.o', '.exe')) for name in members))
        version = subprocess.check_output(['dpkg-deb', '--field', str(self.package), 'Version'], text=True).strip()
        self.assertEqual(version, self.version)

    def test_scripts_are_ascii_executable_and_never_force_module_unload(self):
        for name in ('postinst', 'prerm', 'postrm'):
            script = self.package_root / 'DEBIAN' / name
            data = script.read_bytes()
            self.assertTrue(all(byte in (9, 10, 13) or 32 <= byte <= 126 for byte in data))
            self.assertEqual(script.stat().st_mode & 0o777, 0o755)
            subprocess.run(['sh', '-n', str(script)], check=True)
            commands = [line.strip() for line in data.decode().splitlines() if not line.lstrip().startswith('#')]
            self.assertFalse(any(line.startswith(('modprobe ', 'rmmod ', 'rm ')) for line in commands))

    def test_install_upgrade_and_abort_recovery_delegate_to_distribution_helper(self):
        for action in ('configure', 'abort-upgrade', 'abort-remove', 'abort-deconfigure'):
            with self.subTest(action=action):
                result, calls, _ = self.lifecycle('postinst', action, '0.9.2')
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn(f'common.postinst|openrevo-ec|{self.version}|/usr/share/openrevo-ec-dkms||0.9.2', calls)
                self.assertNotIn('/rmmod|', calls)
                self.assertNotIn('/modprobe|', calls)

    def test_helper_failure_and_missing_helper_stop_configuration(self):
        result, calls, _ = self.lifecycle('postinst', 'configure', helper_rc=42)
        self.assertEqual(result.returncode, 42)
        self.assertNotIn('processing finished', result.stdout)
        self.assertNotIn('/udevadm|', calls)
        result, calls, _ = self.lifecycle('postinst', 'configure', missing_helper=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Missing DKMS', result.stderr)
        self.assertEqual(calls, '')

    def test_manual_module_collision_is_reported_and_preserved(self):
        result, calls, case = self.lifecycle('postinst', 'configure', manual=True)
        self.assertNotEqual(result.returncode, 0)
        legacy = case / 'modules/test-kernel/extra/openrevo/openrevo_ec.ko.zst'
        self.assertIn(str(legacy), result.stderr)
        self.assertEqual(legacy.read_bytes(), b'preserve the manually installed module')
        self.assertEqual(calls, '')

    def test_removal_only_removes_the_exact_registered_version(self):
        for script, action in (('prerm', 'remove'), ('prerm', 'upgrade'),
                               ('prerm', 'deconfigure'), ('postrm', 'purge')):
            with self.subTest(script=script, action=action):
                result, calls, _ = self.lifecycle(script, action, status='registered')
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn(f'dkms|remove|-m|openrevo-ec|-v|{self.version}|--all', calls)
                self.assertNotIn('/rmmod|', calls)
        result, calls, _ = self.lifecycle('prerm', 'remove')
        self.assertEqual(result.returncode, 0)
        self.assertNotIn('|remove|', calls)

    def test_removal_and_status_failures_are_not_hidden(self):
        result, _, _ = self.lifecycle('prerm', 'remove', status_rc=17)
        self.assertEqual(result.returncode, 17)
        result, _, _ = self.lifecycle('prerm', 'remove', status='registered', remove_rc=19)
        self.assertEqual(result.returncode, 19)

    def test_unrelated_lifecycle_actions_do_not_change_dkms_state(self):
        for script, action in (('postinst', 'triggered'), ('prerm', 'failed-upgrade'),
                               ('postrm', 'upgrade')):
            result, calls, _ = self.lifecycle(script, action)
            self.assertEqual(result.returncode, 0)
            self.assertEqual(calls, '')

    def test_packaged_header_is_reproducible_and_transaction_tests_compile(self):
        module = self.package_root / 'usr/src' / f'openrevo-ec-{self.version}'
        regenerated = self.stage / 'regenerated.h'
        subprocess.run(['python3', str(module / 'generate_header.py'),
                        '--registry', str(module / 'config/ec_registry.json'),
                        '--output', str(regenerated)], check=True)
        self.assertEqual(regenerated.read_bytes(), (module / 'vault_generated.h').read_bytes())
        subprocess.run(['make', '-C', str(module), 'test'], check=True, capture_output=True, text=True)

    def test_invalid_versions_are_rejected_before_shell_or_path_use(self):
        for version in ('../bad', '1;exit', '1$(id)', '', '1/2', 'v1.0', '1\nVersion: 2'):
            with self.assertRaises(ValueError):
                PACKAGE.version_value(version)


if __name__ == '__main__':
    unittest.main()
