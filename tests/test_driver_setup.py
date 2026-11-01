"""Exercise install/upgrade outcomes without running host or hardware commands."""
import contextlib
import importlib.machinery
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
LOADER = importlib.machinery.SourceFileLoader('driver_setup', str(ROOT / 'packaging/debian/driver-setup'))
SPEC = importlib.util.spec_from_loader(LOADER.name, LOADER)
SETUP = importlib.util.module_from_spec(SPEC)
LOADER.exec_module(SETUP)
VERSION = json.loads((ROOT / 'src-tauri/tauri.conf.json').read_text())['version']
KERNEL = '6.8.0-test-generic'
SOURCE = 'ABCD1234'


def write(path, content='', executable=False):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding='ascii')
    path.chmod(0o755 if executable else 0o644)


class FakeHost:
    def __init__(self, root, *, loaded=None, environment='host', headers=True,
                 helper_rc=0, busy=False, load_error='', supported=True,
                 session=True, mask='rw-', status=None, shadow=False,
                 writes=True, udev_error=False, environment_error=False):
        self.root, self.loaded, self.environment = root, loaded, environment
        self.helper_rc, self.busy, self.load_error = helper_rc, busy, load_error
        self.session, self.mask, self.shadow = session, mask, shadow
        self.status = status if status is not None else f'openrevo-ec/{VERSION}, {KERNEL}, x86_64: installed\n'
        self.udev_error, self.environment_error = udev_error, environment_error
        self.calls = []
        self.module = root / 'sys/module/openrevo_ec'
        write(root / 'usr/lib/dkms/common.postinst', '#!/bin/sh\nexit 99\n', True)
        write(root / f'lib/modules/{KERNEL}/updates/dkms/openrevo_ec.ko', 'module fixture\n')
        if headers:
            (root / f'lib/modules/{KERNEL}/build/include').mkdir(parents=True)
        if loaded is not None:
            self.load(loaded, writes)
        self.writes = writes
        if supported:
            write(root / 'sys/class/misc/openrevo_ec/dev', '1:3\n')
            (root / 'dev').mkdir()
            # A harmless real character device lets us validate major/minor checks.
            (root / 'dev/openrevo_ec').symlink_to('/dev/null')

    def load(self, version, writes):
        write(self.module / 'srcversion', version + '\n')
        write(self.module / 'parameters/allow_writes', 'Y\n' if writes else 'N\n')

    def __call__(self, args, **kwargs):
        self.calls.append((args, kwargs))
        executable = Path(args[0]).name
        code, output, error = 0, '', ''
        if executable == 'systemd-detect-virt':
            if self.environment_error:
                code, error = 2, 'environment unavailable'
            else:
                code = 0 if args[1] == '--' + self.environment else 1
        elif executable == 'common.postinst':
            code = self.helper_rc
            self.assert_inherited_stdio(kwargs)
            return subprocess.CompletedProcess(args, code, None, None)
        elif executable == 'dkms':
            assert args == ['dkms', 'status', '-m', 'openrevo-ec', '-v', VERSION, '-k', KERNEL]
            output = self.status
        elif executable == 'modinfo':
            if args[4] == 'filename':
                folder = 'extra' if self.shadow else 'updates/dkms'
                output = f'/lib/modules/{KERNEL}/{folder}/openrevo_ec.ko\n'
            else:
                assert args[4] == 'srcversion'
                output = SOURCE + '\n'
        elif executable == 'modprobe':
            assert args in (['modprobe', 'openrevo_ec'], ['modprobe', '-r', 'openrevo_ec'])
            if args[1] == '-r':
                if self.busy:
                    code, error = 1, 'Module is in use'
                else:
                    self.module.rename(self.root / 'retired-module')
            elif self.load_error:
                code, error = 1, self.load_error
            else:
                self.load(SOURCE, self.writes)
        elif executable == 'udevadm':
            if self.udev_error:
                code, error = 1, 'udev unavailable'
            elif args[1] == 'info':
                output = 'TAGS=:uaccess:seat:\nID_SEAT=seat0\n'
            elif args[1] == 'trigger':
                assert args == ['udevadm', 'trigger', '--action=add', str(self.root / 'sys/class/misc/openrevo_ec')]
        elif executable == 'loginctl':
            if args[1] == 'show-seat':
                output = '2\n' if self.session else ''
            else:
                output = 'User=1000\nActive=yes\nRemote=no\n'
        elif executable == 'getfacl':
            output = f'user::rw-\nuser:1000:rw-\ngroup::---\nmask::{self.mask}\nother::---\n'
        else:
            raise AssertionError(f'Unexpected command: {args}')
        return subprocess.CompletedProcess(args, code, output, error)

    @staticmethod
    def assert_inherited_stdio(options):
        assert options['stdout'] is None and options['stderr'] is None
        assert options['timeout'] is None
        assert 'stdin' not in options


class DriverSetupTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        release = ROOT / 'dist_release'
        release.mkdir(exist_ok=True)
        cls.output = Path(tempfile.mkdtemp(prefix='driver-setup-tests-', dir=release))

    def fixture(self, **options):
        root = Path(tempfile.mkdtemp(prefix='host-', dir=self.output))
        host = FakeHost(root, **options)
        setup = SETUP.DriverSetup(VERSION, '0.9.3-linux.1', root=root, runner=host, kernel=KERNEL)
        return root, host, setup

    def configure(self, setup, expected, code=0):
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            result = setup.configure()
        data = json.loads(setup.path(SETUP.STATUS_PATH).read_text())
        self.assertEqual(result, code, data)
        self.assertEqual(data['state'], expected, data)
        self.assertEqual(data['ready'], expected == 'ready')
        self.assertNotIn('@VERSION@', data['message'])
        return data

    def no_activation(self, host):
        self.assertFalse(any(Path(args[0]).name in ('modprobe', 'udevadm', 'loginctl', 'getfacl')
                             for args, _ in host.calls))

    def test_first_install_activates_driver_and_grants_local_user_access(self):
        _, host, setup = self.fixture()
        data = self.configure(setup, 'ready')
        self.assertTrue(data['module_active'])
        self.assertEqual(data['active_uid'], 1000)
        self.assertEqual(data['running_srcversion'], SOURCE)
        self.assertIn(['modprobe', 'openrevo_ec'], [args for args, _ in host.calls])

    def test_identical_loaded_module_and_repeated_configuration_do_not_reload(self):
        _, host, setup = self.fixture(loaded=SOURCE)
        self.configure(setup, 'ready')
        self.configure(setup, 'ready')
        self.assertFalse(any(args[0] == 'modprobe' for args, _ in host.calls))

    def test_changed_module_is_replaced_using_normal_unload(self):
        _, host, setup = self.fixture(loaded='0000')
        self.configure(setup, 'ready')
        commands = [args for args, _ in host.calls if args[0] == 'modprobe']
        self.assertEqual(commands, [['modprobe', '-r', 'openrevo_ec'], ['modprobe', 'openrevo_ec']])

    def test_busy_module_is_preserved_and_requests_reboot(self):
        _, host, setup = self.fixture(loaded='0000', busy=True)
        self.configure(setup, 'reboot-required')
        self.assertEqual((host.module / 'srcversion').read_text().strip(), '0000')
        self.assertNotIn(['modprobe', 'openrevo_ec'], [args for args, _ in host.calls])

    def test_chroots_and_containers_never_activate_or_touch_host_devices(self):
        for environment in ('chroot', 'container'):
            with self.subTest(environment=environment):
                _, host, setup = self.fixture(environment=environment)
                self.configure(setup, 'deferred-environment')
                self.no_activation(host)

    def test_unsafe_isolated_dkms_hooks_are_rejected_before_building(self):
        root, host, setup = self.fixture(environment='chroot')
        write(root / 'etc/dkms/framework.conf', 'modprobe_on_install="yes"\n')
        self.configure(setup, 'isolation-hook-conflict', 1)
        self.assertFalse(any(Path(args[0]).name == 'common.postinst' for args, _ in host.calls))
        self.no_activation(host)

    def test_uncertain_environment_cannot_activate_modules(self):
        _, host, setup = self.fixture(environment_error=True)
        self.configure(setup, 'environment-unknown', 1)
        self.no_activation(host)

    def test_missing_headers_and_disabled_autoinstall_remain_actionable(self):
        _, host, setup = self.fixture(headers=False)
        data = self.configure(setup, 'headers-missing')
        self.assertIn('driver-setup configure ' + VERSION, data['message'])
        self.no_activation(host)
        root, host, setup = self.fixture()
        write(root / 'etc/dkms/no-autoinstall')
        self.configure(setup, 'autoinstall-disabled')
        self.assertFalse(any(Path(args[0]).name == 'common.postinst' for args, _ in host.calls))

    def test_real_dkms_failure_and_incomplete_registration_fail_configuration(self):
        _, host, setup = self.fixture(helper_rc=42)
        self.configure(setup, 'dkms-build-failed', 1)
        self.no_activation(host)
        _, host, setup = self.fixture(status=f'openrevo-ec/{VERSION}, {KERNEL}, x86_64: built\n')
        self.configure(setup, 'module-not-installed', 1)
        self.no_activation(host)

    def test_manual_duplicate_is_preserved_and_shadowed_module_is_not_loaded(self):
        root, host, setup = self.fixture()
        manual = root / f'lib/modules/{KERNEL}/extra/openrevo/openrevo_ec.ko.zst'
        write(manual, 'keep existing module\n')
        self.configure(setup, 'manual-module-conflict', 1)
        self.assertEqual(manual.read_text(), 'keep existing module\n')
        self.no_activation(host)
        _, host, setup = self.fixture(shadow=True)
        self.configure(setup, 'module-shadowed', 1)
        self.no_activation(host)

    def test_secure_boot_rejection_is_actionable_but_other_load_errors_fail(self):
        _, _, setup = self.fixture(load_error='Key was rejected by service')
        self.configure(setup, 'secure-boot-action-required')
        _, _, setup = self.fixture(load_error='Invalid module format')
        self.configure(setup, 'module-load-failed', 1)

    def test_unsupported_hardware_read_only_and_no_session_do_not_block_app_install(self):
        for options, state in (({'supported': False}, 'unsupported-hardware'),
                               ({'writes': False}, 'device-read-only'),
                               ({'session': False}, 'awaiting-session')):
            with self.subTest(state=state):
                _, _, setup = self.fixture(**options)
                self.configure(setup, state)

    def test_acl_mask_and_udev_failures_cannot_report_ready(self):
        _, _, setup = self.fixture(mask='r--')
        self.configure(setup, 'permissions-not-ready', 1)
        _, _, setup = self.fixture(udev_error=True)
        self.configure(setup, 'udev-failed', 1)

    def test_mismatched_device_node_cannot_report_ready(self):
        root, _, setup = self.fixture()
        write(root / 'sys/class/misc/openrevo_ec/dev', '1:5\n')
        self.configure(setup, 'device-node-mismatch', 1)

    def test_postinst_handles_real_dpkg_abort_arguments(self):
        case = Path(tempfile.mkdtemp(prefix='postinst-', dir=self.output))
        helper = case / 'helper'
        log = case / 'arguments'
        write(helper, '#!/bin/sh\nprintf "%s\\n" "$@" > "$TEST_LOG"\n', True)
        script = case / 'postinst'
        script.write_text((ROOT / 'packaging/debian/postinst.in').read_text()
                          .replace('@VERSION@', VERSION)
                          .replace('/usr/lib/open-revo/driver-setup', str(helper)))
        for arguments, previous in ((['configure', '0.9.3-linux.1'], '0.9.3-linux.1'),
                                    (['abort-upgrade', '0.9.3-linux.3'], '0.9.3-linux.3'),
                                    (['abort-remove', 'in-favour', 'other', '1.0'], ''),
                                    (['abort-deconfigure', 'in-favour', 'other', '1.0'], '')):
            with self.subTest(arguments=arguments):
                subprocess.run(['/bin/sh', str(script)] + arguments, check=True,
                               env=dict(os.environ, TEST_LOG=str(log)))
                self.assertEqual(log.read_text().splitlines(), ['configure', VERSION, previous])


if __name__ == '__main__':
    unittest.main()
