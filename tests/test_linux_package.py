"""Hardware-free checks for the complete package and split-package migration."""
import importlib.util
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('linux_package', ROOT / 'packaging/build_linux_package.py')
PACKAGE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PACKAGE)


class LinuxPackageTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        release = ROOT / 'dist_release'
        release.mkdir(exist_ok=True)
        cls.output = Path(tempfile.mkdtemp(prefix='linux-package-tests-', dir=release))
        cls.version = PACKAGE.DRIVER.default_version()
        app_root = cls.output / 'app'
        PACKAGE.DRIVER.write(app_root / 'usr/bin/open-revo', '#!/bin/sh\nexit 0\n', True)
        PACKAGE.DRIVER.write(app_root / 'usr/share/applications/OpenRevo.desktop',
                             '[Desktop Entry]\nName=OpenRevo\nExec=open-revo\nType=Application\n')
        PACKAGE.DRIVER.write(app_root / 'DEBIAN/control', f'''Package: open-revo
Version: {cls.version}
Architecture: amd64
Maintainer: Test <test@example.invalid>
Description: Application fixture
Depends: libwebkit2gtk-4.1-0, libayatana-appindicator3-1, libwebkit2gtk-4.1-0
''')
        cls.app = cls.output / 'application.deb'
        cls.deb(app_root, cls.app)
        cls.package, cls.stage = PACKAGE.build(cls.app, cls.output)
        cls.package_root = cls.stage / 'root'

    @staticmethod
    def deb(root, output):
        subprocess.run(['dpkg-deb', '--root-owner-group', '--build', str(root), str(output)],
                       check=True, capture_output=True)

    def fixture(self):
        case = Path(tempfile.mkdtemp(prefix='case-', dir=self.output))
        root = case / 'package'
        shutil.copytree(self.package_root, root)
        return case, root

    def test_single_package_has_app_driver_dependencies_and_split_migration(self):
        control = PACKAGE.verify(self.package)
        dependencies = [part.strip() for part in control['Depends'].split(',')]
        self.assertEqual(len(dependencies), len(set(dependencies)))
        self.assertNotIn('openrevo-ec-dkms', dependencies)
        self.assertIn('linux-headers-generic | linux-headers-amd64 | linux-headers', dependencies)
        self.assertIn('dkms (>= 2.8.7)', dependencies)
        self.assertEqual(control['Conflicts'], 'openrevo-ec-dkms')
        self.assertEqual(control['Replaces'], 'openrevo-ec-dkms')
        self.assertEqual(control['Provides'], f'openrevo-ec-dkms (= {self.version})')

    def test_missing_driver_dependency_is_rejected(self):
        case, root = self.fixture()
        control = root / 'DEBIAN/control'
        control.write_text(control.read_text().replace('dkms (>= 2.8.7), ', ''))
        package = case / 'broken.deb'
        self.deb(root, package)
        with self.assertRaisesRegex(ValueError, 'Missing dependency: dkms'):
            PACKAGE.verify(package)

    def test_payload_corruption_is_rejected(self):
        case, root = self.fixture()
        (root / 'usr/bin/open-revo').write_text('corrupted application\n')
        package = case / 'broken.deb'
        self.deb(root, package)
        with self.assertRaisesRegex(ValueError, 'checksums'):
            PACKAGE.verify(package)

    def test_old_version_and_already_complete_input_are_rejected(self):
        case = Path(tempfile.mkdtemp(prefix='old-version-', dir=self.output))
        root = case / 'app'
        subprocess.run(['dpkg-deb', '--raw-extract', str(self.app), str(root)], check=True)
        control = root / 'DEBIAN/control'
        control.write_text(control.read_text().replace(self.version, '0.0.1'))
        package = case / 'old.deb'
        self.deb(root, package)
        with self.assertRaisesRegex(ValueError, 'Expected Version'):
            PACKAGE.build(package, case)
        with self.assertRaisesRegex(ValueError, 'application-only'):
            PACKAGE.build(self.package, case)

    def test_output_cannot_escape_release_directory(self):
        with self.assertRaisesRegex(ValueError, 'dist_release'):
            PACKAGE.build(self.app, ROOT)

    def test_real_dpkg_migrates_split_ownership_and_preserves_local_configuration(self):
        # Use an isolated database/root. Replace hooks in these fixtures with logs;
        # the production hooks are tested separately, without changing host DKMS.
        case, root = self.fixture()
        install_root = case / 'installed'
        install_root.mkdir()
        log_script = '''#!/bin/sh
set -eu
printf '%s:%s\\n' "$DPKG_MAINTSCRIPT_PACKAGE/$DPKG_MAINTSCRIPT_NAME" "$*" >> "$DPKG_ROOT/lifecycle.log"
'''
        for name in ('postinst', 'prerm', 'postrm'):
            PACKAGE.DRIVER.write(root / 'DEBIAN' / name, log_script, True)
        new_package = case / 'complete.deb'
        self.deb(root, new_package)
        old_version = '0.9.3-linux.1'
        driver_root = case / 'old-driver'
        PACKAGE.DRIVER.write(driver_root / 'DEBIAN/control', f'''Package: openrevo-ec-dkms
Version: {old_version}
Architecture: all
Maintainer: Test <test@example.invalid>
Description: Previous split driver fixture
''')
        PACKAGE.DRIVER.write(driver_root / 'DEBIAN/conffiles', '\n'.join(PACKAGE.CONFFILES) + '\n')
        for path in PACKAGE.CONFFILES:
            PACKAGE.DRIVER.write(driver_root / path.lstrip('/'), (root / path.lstrip('/')).read_text())
        source = f'usr/src/openrevo-ec-{old_version}/dkms.conf'
        PACKAGE.DRIVER.write(driver_root / source, 'old driver source\n')
        rule = 'usr/lib/udev/rules.d/70-openrevo-ec.rules'
        PACKAGE.DRIVER.write(driver_root / rule, (root / rule).read_text())
        for name in ('postinst', 'prerm', 'postrm'):
            PACKAGE.DRIVER.write(driver_root / 'DEBIAN' / name, log_script, True)
        old_driver = case / 'driver.deb'
        self.deb(driver_root, old_driver)
        old_app_root = case / 'old-app'
        subprocess.run(['dpkg-deb', '--raw-extract', str(self.app), str(old_app_root)], check=True)
        control = old_app_root / 'DEBIAN/control'
        control.write_text(control.read_text().replace(self.version, old_version))
        old_app = case / 'app.deb'
        self.deb(old_app_root, old_app)
        dpkg = ['dpkg', '--root=' + str(install_root), '--force-not-root',
                '--force-script-chrootless', '--force-depends', '--force-confold']
        environment = dict(os.environ, DPKG_COLORS='never')

        def run(*args):
            result = subprocess.run(dpkg + list(args), text=True, capture_output=True, env=environment)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            return result.stdout

        run('--install', str(old_driver), str(old_app))
        config = install_root / 'etc/modprobe.d/openrevo-ec.conf'
        custom = config.read_text() + '# administrator setting preserved\n'
        config.write_text(custom)
        unrelated = install_root / 'etc/unrelated.conf'
        unrelated.write_text('keep\n')
        run('--install', str(new_package))
        self.assertEqual(config.read_text(), custom)
        status = (install_root / 'var/lib/dpkg/status').read_text()
        self.assertIn('Package: open-revo\nStatus: install ok installed', status)
        self.assertNotIn('Package: openrevo-ec-dkms\nStatus: install ok installed', status)
        self.assertFalse((install_root / source).exists())
        self.assertTrue((install_root / f'usr/src/openrevo-ec-{self.version}/dkms.conf').is_file())
        self.assertTrue((install_root / rule).is_file())
        self.assertIn('openrevo-ec-dkms/prerm:remove', (install_root / 'lifecycle.log').read_text())
        # The old package's later purge must not remove the new owner's conffiles.
        run('--purge', 'openrevo-ec-dkms')
        self.assertEqual(config.read_text(), custom)
        run('--remove', 'open-revo')
        self.assertEqual(config.read_text(), custom)
        self.assertFalse((install_root / 'usr/bin/open-revo').exists())
        run('--purge', 'open-revo')
        self.assertFalse(config.exists())
        self.assertEqual(unrelated.read_text(), 'keep\n')


if __name__ == '__main__':
    unittest.main()
