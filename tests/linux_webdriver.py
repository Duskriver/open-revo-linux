#!/usr/bin/env python3
"""Smoke test the actual Linux Tauri/WebKit window using W3C WebDriver.

Requires tauri-driver, WebKitWebDriver and a display (or xvfb-run).
Hardware writes are disabled by default. Set OPENREVO_TEST_HARDWARE_WRITES=1
only for an explicitly authorized current-value write/readback check.
UI fixtures never write hardware or change login startup preferences.
"""
import base64
from contextlib import contextmanager
from http.client import HTTPException
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import unittest
from urllib.error import URLError
from urllib.request import Request, urlopen

ROOT = Path(__file__).resolve().parents[1]
PORT = int(os.environ.get("OPENREVO_WEBDRIVER_PORT", "4444"))
URL = f"http://127.0.0.1:{PORT}"
HARDWARE_WRITES = os.environ.get("OPENREVO_TEST_HARDWARE_WRITES") == "1"


def request(method, path, data=None):
    body = None if data is None else json.dumps(data).encode()
    req = Request(URL + path, body, {"Content-Type": "application/json"}, method=method)
    with urlopen(req, timeout=30) as response:
        result = json.load(response)
    value = result.get("value")
    if isinstance(value, dict) and "error" in value:
        raise RuntimeError(value)
    return value


class LinuxWindow(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        driver = shutil.which("tauri-driver")
        if not driver:
            version = json.loads((ROOT / "tests/toolchain.json").read_text())["tauri-driver"]
            raise RuntimeError(f"Install tauri-driver with cargo install tauri-driver --version {version} --locked")
        binary = Path(os.environ.get("OPENREVO_TEST_BINARY", ROOT / "src-tauri/target/debug/open-revo")).resolve()
        if not binary.is_file():
            raise RuntimeError("Build the GUI first: pnpm tauri build --debug --no-bundle")
        environment = os.environ.copy()
        if environment.get('OPENREVO_TEST_SCREENSHOT_BACKEND') == 'x11':
            # Keep the app on the same Xvfb display as the capture process,
            # even when this test is launched from a Wayland desktop.
            environment.pop('WAYLAND_DISPLAY', None)
            environment['GDK_BACKEND'] = 'x11'
        # Keep both native settings and WebKit localStorage away from the user's
        # application data. Retain every run's directory for failure inspection.
        output_root = ROOT / "test-results"
        output_root.mkdir(exist_ok=True)
        cls.test_home = Path(tempfile.mkdtemp(prefix="linux-webdriver-", dir=output_root))
        print(f"Isolated XDG home and retained artifacts: {cls.test_home}", flush=True)
        for name, child in [("XDG_CONFIG_HOME", "config"), ("XDG_DATA_HOME", "data"),
                            ("XDG_CACHE_HOME", "cache")]:
            environment[name] = str(cls.test_home / child)
        cls.config_directory = cls.test_home / "config" / "OpenRevo"
        cls.config_file = cls.config_directory / "config.json"
        if environment.get("DISPLAY") and not environment.get("WAYLAND_DISPLAY"):
            # Virtual X servers have no DRI3/DMABUF device. Keep software
            # rendering local to the test process and its WebKit children.
            environment.setdefault("LIBGL_ALWAYS_SOFTWARE", "1")
            environment.setdefault("WEBKIT_DISABLE_DMABUF_RENDERER", "1")
        cls.process = subprocess.Popen([driver, "--port", str(PORT)], env=environment)
        cls.addClassCleanup(cls.stop_driver)
        for _ in range(100):
            try:
                result = request("POST", "/session", {"capabilities": {"alwaysMatch": {
                    "browserName": "wry", "tauri:options": {"application": str(binary)}
                }}})
                cls.session = result["sessionId"]
                break
            except (URLError, HTTPException, OSError):
                time.sleep(0.1)
        else:
            raise RuntimeError("Could not start WebDriver")
        cls.addClassCleanup(lambda: request("DELETE", f"/session/{cls.session}"))
        cls.execute("document.documentElement.lang = 'zh-CN'; return true;")
        cls.execute("""
            window.__testHardwareCommands = [
              'set_linux_power_profile', 'set_linux_platform_profile', 'set_linux_fan_boost',
              'set_linux_power_limit', 'set_linux_oem_mode', 'set_linux_oem_fan_boost',
              'set_linux_oem_cpu_limits', 'set_linux_oem_fan_curve', 'restore_linux_oem_fan_curve',
              'set_linux_battery_limit', 'set_linux_backlight', 'set_autostart_enabled',
              'renew_linux_control_lease'
            ]; return true;
        """)
        for _ in range(100):
            if "已连接本机" in cls.execute("return document.body.innerText;"):
                break
            time.sleep(0.1)
        else:
            raise RuntimeError("Hardware snapshot did not reach the native window: " + cls.execute("return document.body.innerText;"))

    @classmethod
    def stop_driver(cls):
        cls.process.terminate()
        try:
            cls.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            cls.process.kill()
            cls.process.wait()

    @classmethod
    def execute(cls, script, args=None):
        return request("POST", f"/session/{cls.session}/execute/sync", {"script": script, "args": args or []})

    def wait_js(self, expression, timeout=10):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            result = self.execute(f"return ({expression});")
            if result:
                return result
            time.sleep(0.1)
        self.fail(f"Timed out waiting for {expression}: " + self.execute("return document.body.innerText;"))

    def navigate(self, name):
        self.execute("[...document.querySelectorAll('[role=tab]')].find(b => b.textContent.trim() === arguments[0]).click(); return true;", [name])

    def screenshot(self):
        backend = os.environ.get('OPENREVO_TEST_SCREENSHOT_BACKEND', 'webdriver')
        if backend == 'x11':
            from x11_screenshot import capture
            return capture(request('GET', f'/session/{self.session}/window/rect'))
        if backend != 'webdriver':
            raise RuntimeError(f'Unknown screenshot backend: {backend}')
        # WebKitWebDriver can time out a capture during a compositor update.
        # Retry only this read-only request; never repeat an IPC mutation.
        for attempt in range(2):
            try:
                return base64.b64decode(request('GET', f'/session/{self.session}/screenshot'))
            except (TimeoutError, URLError, HTTPException, OSError):
                if attempt:
                    raise
                print('Screenshot transport failed; retrying once', flush=True)
                time.sleep(0.3)

    def click_text(self, text):
        self.wait_js(f"[...document.querySelectorAll('button')].some(b => b.textContent === {json.dumps(text)} && !b.disabled)")
        self.execute("[...document.querySelectorAll('button')].find(b => b.textContent === arguments[0]).click(); return true;", [text])

    def native_ipc(self, command, arguments=None):
        self.execute("""
            window.__ipcResult = null;
            window.__TAURI_INTERNALS__.invoke(arguments[0], arguments[1])
              .then(value => window.__ipcResult = {ok:true, value})
              .catch(rejection => window.__ipcResult = {ok:false, rejection});
            return true;
        """, [command, arguments or {}])
        return self.wait_js("window.__ipcResult")

    @contextmanager
    def isolated_config_document(self):
        """Temporarily archive only the two known files inside this test run."""
        self.navigate("状态总览")
        archive = self.test_home / f"config-case-{time.time_ns()}"
        (archive / "before").mkdir(parents=True)
        (archive / "after").mkdir()
        self.config_directory.mkdir(parents=True, exist_ok=True)
        names = ("config.json", "config.json.bak")
        for name in names:
            source = self.config_directory / name
            if source.exists():
                source.rename(archive / "before" / name)
        try:
            yield
        finally:
            self.navigate("状态总览")
            for name in names:
                current = self.config_directory / name
                if current.exists():
                    current.rename(archive / "after" / name)
                previous = archive / "before" / name
                if previous.exists():
                    previous.rename(current)

    @staticmethod
    def valid_draft(first_duty=22, ramp_ms=300):
        points = [{"temp": temp, "duty": duty} for temp, duty in
                  zip([40, 50, 60, 68, 75, 82, 88, 95], [first_duty, 30, 45, 55, 65, 75, 85, 100])]
        return {"cpu": points, "gpu": [dict(point) for point in points],
                "isolated": True, "ramp_enabled": True, "ramp_ms": ramp_ms}

    def install_curve_fixture(self, active=False):
        self.execute("""
            window.__curveReady = false;
            const active = arguments[0], draft = arguments[1];
            (async () => {
              const snapshot = await window.__TAURI_INTERNALS__.invoke('get_linux_status');
              snapshot.model = 'Fan curve UI fixture';
              snapshot.ac_online = true;
              snapshot.oem = {...snapshot.oem, available:true, writable:true, error:null,
                mode:1, custom_power:false, fan_boost:false, dc_jack:true, fan_rpm:[2500,2600],
                presets:[{pl1:35,pl2:35,pl4:70},{pl1:65,pl2:65,pl4:100},{pl1:90,pl2:90,pl4:100}],
                limits:{pl1:35,pl2:35,pl4:70},
                fan_curve:{supported:true, active, fault:false, applied:active ? draft : null}};
              window.__curveFixture = snapshot;
              window.__curveWrites = [];
              window.__fixtureRenewals = 0;
              window.__fixtureConfigReads = 0;
              window.__fixtureConfigWrites = 0;
              window.__statusHeld = false;
              window.__statusWaiters = [];
              window.__delaySave = false;
              window.__releaseSave = null;
              const nativeFetch = window.fetch;
              window.__nativeFetch = nativeFetch;
              const commands = ['get_linux_status', 'get_linux_config', 'save_linux_fan_curve_draft',
                ...window.__testHardwareCommands];
              const urls = Object.fromEntries(commands.map(c => [window.__TAURI_INTERNALS__.convertFileSrc(c,'ipc'),c]));
              const response = value => new Response(JSON.stringify(value ?? null),
                {headers:{'Content-Type':'application/json','Tauri-Response':'ok'}});
              window.fetch = async (url, options) => {
                const command = urls[String(url)];
                if (!command) return nativeFetch(url, options);
                if (command === 'get_linux_config') {
                  window.__fixtureConfigReads++;
                  return nativeFetch(url, options);
                }
                if (command === 'save_linux_fan_curve_draft') {
                  window.__fixtureConfigWrites++;
                  if (window.__delaySave) await new Promise(resolve => { window.__releaseSave = resolve; });
                  return nativeFetch(url, options);
                }
                if (command === 'get_linux_status') {
                  if (window.__statusHeld) await new Promise(resolve => window.__statusWaiters.push(resolve));
                  snapshot.timestamp_ms = Date.now();
                  return response(structuredClone(snapshot));
                }
                if (command === 'renew_linux_control_lease') {
                  window.__fixtureRenewals++;
                  return response(null);
                }
                const args = JSON.parse(options.body);
                window.__curveWrites.push({command,args});
                if (command === 'set_linux_oem_fan_curve') {
                  snapshot.oem.fan_curve.active = true;
                  snapshot.oem.fan_curve.applied = args.config;
                } else if (command === 'restore_linux_oem_fan_curve') {
                  snapshot.oem.fan_curve.active = false;
                  snapshot.oem.fan_curve.applied = null;
                } else {
                  return new Response(JSON.stringify('Unexpected hardware write blocked by fixture'),
                    {headers:{'Content-Type':'application/json','Tauri-Response':'error'}});
                }
                return response(null);
              };
              window.__curveReady = true;
            })().catch(error => window.__curveFixtureError = String(error));
            return true;
        """, [active, self.valid_draft()])
        self.wait_js("window.__curveReady")
        self.execute("document.querySelector('[aria-label=刷新]').click(); return true;")
        self.wait_for_status_fixture("Fan curve UI fixture")

    @contextmanager
    def curve_fixture(self, active=False):
        self.install_curve_fixture(active)
        try:
            yield
        finally:
            self.clear_curve_fixture()

    def clear_curve_fixture(self):
        self.execute("""
            window.__statusHeld = false;
            for (const release of (window.__statusWaiters ?? []).splice(0)) release();
            if (window.__releaseSave) { window.__releaseSave(); window.__releaseSave = null; }
            if (window.__nativeFetch) window.fetch = window.__nativeFetch;
            [...document.querySelectorAll('.linux-error button')].forEach(button => button.click());
            document.querySelector('[aria-label=刷新]').click();
            [...document.querySelectorAll('[role=tab]')].find(b => b.textContent.trim() === '状态总览').click();
            return true;
        """)

    def wait_for_status_fixture(self, model):
        # Installing the fetch interceptor does not finish an in-flight native
        # poll. Wait for this fixture to render before asserting its controls.
        text = ""
        for _ in range(100):
            text = self.execute("return document.querySelector('.linux-status')?.textContent ?? '';")
            if model in text:
                return
            time.sleep(0.1)
        self.fail(f"Status fixture {model!r} did not reach the UI: {text}")

    def test_native_hardware_snapshot_and_capabilities(self):
        text = self.execute("return document.body.innerText;")
        self.assertIn("OPENREVO", text)
        self.assertIn("性能释放模式", text)
        self.assertIn("风扇与散热控制", text)
        self.assertNotIn("启动失败", text)
        self.assertEqual(self.execute("return !!document.querySelector('.linux-shell');"), True)
        self.assertEqual(self.execute("return document.querySelectorAll('.shared-mode-grid button').length;"), 3)

    def test_shared_navigation_and_power_page(self):
        self.assertTrue(self.execute("return !!document.querySelector('.shell-navbar .sub-nav-bar');"))
        self.assertEqual(self.execute("return document.querySelectorAll('.telemetry-cards-grid .telemetry-card').length;"), 4)
        self.execute("[...document.querySelectorAll('[role=tab]')].find(b => b.textContent.trim() === '电源管理').click(); return true;")
        text = self.execute("return document.body.innerText;")
        self.assertTrue("硬件功耗调校" in text or "CPU 功耗调校" in text)
        self.assertIn("风扇与散热控制", text)
        self.assertNotIn("运行出错", text)
        self.execute("[...document.querySelectorAll('[role=tab]')].find(b => b.textContent.trim() === '系统').click(); return true;")
        self.assertIn("硬件支持状态", self.execute("return document.body.innerText;"))
        self.assertTrue(self.execute("return !!document.querySelector('.linux-telemetry-sources');"))
        self.execute("document.querySelector('.linux-telemetry-sources summary').click(); document.querySelector('.linux-telemetry-sources').scrollIntoView({block:'start'}); return true;")
        self.assertGreater(self.execute("return document.querySelectorAll('.linux-telemetry-sources .quick-row').length;"), 0)
        time.sleep(0.2)
        (self.test_home / "telemetry-sources.png").write_bytes(
            self.screenshot())
        self.execute("[...document.querySelectorAll('[role=tab]')].find(b => b.textContent.trim() === '状态总览').click(); return true;")

    def test_new_control_commands_reject_unknown_targets(self):
        self.execute("""
            window.__controlErrors = null;
            Promise.all([
              ['set_linux_platform_profile', {profile: 'invalid'}],
              ['set_linux_fan_boost', {id: '../../etc/passwd', enabled: true}],
              ['set_linux_power_limit', {id: '../../etc/passwd', watts: 30}]
            ].map(([command, args]) => window.__TAURI_INTERNALS__.invoke(command, args)
              .then(() => 'unexpected success').catch(error => String(error))))
              .then(errors => window.__controlErrors = errors);
            return true;
        """)
        for _ in range(100):
            errors = self.execute("return window.__controlErrors;")
            if errors is not None:
                self.assertEqual(len(errors), 3)
                self.assertTrue(all('未' in error for error in errors), errors)
                return
            time.sleep(0.1)
        self.fail("Control validation did not complete")

    def test_theme_switch_and_restore(self):
        initial = self.execute("return document.documentElement.dataset.theme;")
        self.assertEqual(self.execute("return getComputedStyle(document.documentElement).colorScheme;"), "dark")
        self.execute("document.querySelector('[aria-label=\"切换主题\"]').click(); return true;")
        time.sleep(0.15)
        self.assertNotEqual(self.execute("return document.documentElement.dataset.theme;"), initial)
        self.assertEqual(self.execute("return getComputedStyle(document.documentElement).colorScheme;"), "dark")
        self.execute("document.querySelector('[aria-label=\"切换主题\"]').click(); return true;")
        time.sleep(0.15)
        self.assertEqual(self.execute("return document.documentElement.dataset.theme;"), initial)

    def test_native_ipc_rejects_invalid_profile(self):
        # Exercise the real command boundary; invalid input fails before writing.
        self.execute("""
            window.__invalidProfileResult = null;
            window.__TAURI_INTERNALS__.invoke('set_linux_power_profile', {profile: 'invalid'})
              .then(() => window.__invalidProfileResult = 'unexpected success')
              .catch(error => window.__invalidProfileResult = String(error));
            return true;
        """)
        for _ in range(50):
            result = self.execute("return window.__invalidProfileResult;")
            if result is not None:
                self.assertIn("无效性能档位", result)
                return
            time.sleep(0.1)
        self.fail("Native IPC command did not complete")

    @unittest.skipUnless(HARDWARE_WRITES, "Real profile writes require explicit opt-in")
    def test_native_profile_permissions_and_readback(self):
        self.execute("""
            window.__profileReadback = null;
            (async () => {
              const invoke = window.__TAURI_INTERNALS__.invoke;
              const before = await invoke('get_linux_status');
              if (!before.power.active) { window.__profileReadback = 'unavailable'; return; }
              try {
                await invoke('set_linux_power_profile', {profile: before.power.active});
              } catch (error) {
                if (before.power.can_set === false && /AccessDenied|NotAuthorized/.test(String(error))) {
                  window.__profileReadback = 'permission-denied'; return;
                }
                throw error;
              }
              const after = await invoke('get_linux_status');
              window.__profileReadback = after.power.active === before.power.active ? 'ok' : 'mismatch';
            })().catch(error => window.__profileReadback = String(error));
            return true;
        """)
        for _ in range(150):
            result = self.execute("return window.__profileReadback;")
            if result is not None:
                if result == 'unavailable':
                    self.skipTest('No power profiles service on this machine')
                self.assertIn(result, ['ok', 'permission-denied'])
                if result == 'permission-denied':
                    self.assertTrue(self.execute("return [...document.querySelectorAll('.shared-mode-grid button')].every(b => b.disabled);"))
                return
            time.sleep(0.1)
        self.fail("Profile write/readback did not complete")

    @unittest.skipUnless(HARDWARE_WRITES, "Real platform writes require explicit opt-in")
    def test_platform_profile_current_value_readback(self):
        # Only write the current value when both policy layers already agree.
        self.execute("""
            window.__platformReadback = null;
            (async () => {
              const invoke = window.__TAURI_INTERNALS__.invoke;
              const before = await invoke('get_linux_status');
              const profile = before.controls.platform.active;
              if (!profile) { window.__platformReadback = 'unavailable'; return; }
              const mapped = {'low-power': 'power-saver', balanced: 'balanced', performance: 'performance'}[profile];
              if (before.power.available.length && before.power.active !== mapped) {
                window.__platformReadback = 'different-policy'; return;
              }
              if (before.power.can_set === false || (!before.power.available.length && !before.controls.platform.writable)) {
                window.__platformReadback = 'read-only'; return;
              }
              await invoke('set_linux_platform_profile', {profile});
              const after = await invoke('get_linux_status');
              window.__platformReadback = after.controls.platform.active === profile ? 'ok' : 'mismatch';
            })().catch(error => window.__platformReadback = String(error));
            return true;
        """)
        for _ in range(150):
            result = self.execute("return window.__platformReadback;")
            if result is not None:
                if result in ['unavailable', 'read-only', 'different-policy']:
                    self.skipTest(result)
                self.assertEqual(result, 'ok')
                return
            time.sleep(0.1)
        self.fail('Platform profile write/readback did not complete')

    def test_ui_control_flow_with_fixtures(self):
        # UI-only fixture: intercepted writes never reach hardware. Native IPC
        # and real sysfs discovery are exercised separately above.
        self.execute("""
            window.__fixtureReady = false;
            (async () => {
              const nativeInvoke = window.__TAURI_INTERNALS__.invoke;
              const snapshot = await nativeInvoke('get_linux_status');
              snapshot.oem.available = false;
              snapshot.power = {active: null, available: [], source: null, can_set: null, error: null};
              snapshot.controls.platform = {active: 'balanced', available: ['low-power', 'balanced', 'performance'], source: 'UI fixture', writable: true};
              snapshot.controls.fans = [{id: 'fixture-fan', name: 'Fixture fan', rpm: 2500, mode: 3, boosted: false, writable: true, reason: null}];
              snapshot.controls.power_limits = [{id: 'fixture-power', name: 'Fixture CPU', watts: 45, min_watts: 10, max_watts: 65, writable: true, reason: null}];
              window.__fixtureWrites = [];
              const nativeFetch = window.fetch;
              window.__nativeFetch = nativeFetch;
              const commands = ['get_linux_status', ...window.__testHardwareCommands];
              const urls = Object.fromEntries(commands.map(cmd => [window.__TAURI_INTERNALS__.convertFileSrc(cmd, 'ipc'), cmd]));
              const handle = async (command, args) => {
                if (command === 'get_linux_status') return structuredClone(snapshot);
                if (command === 'renew_linux_control_lease') return null;
                if (command.startsWith('set_linux_')) {
                  window.__fixtureWrites.push({command, args});
                  if (command === 'set_linux_fan_boost') {
                    snapshot.controls.fans[0].boosted = args.enabled;
                    snapshot.controls.fans[0].mode = args.enabled ? 0 : 3;
                  } else if (command === 'set_linux_power_limit') {
                    if (args.watts < 10 || args.watts > 65) throw 'out of bounds';
                    snapshot.controls.power_limits[0].watts = args.watts;
                  } else if (command === 'set_linux_platform_profile') snapshot.controls.platform.active = args.profile;
                  else throw 'unexpected fixture command';
                  return;
                }
                throw 'unexpected fixture command';
              };
              window.fetch = async (url, options) => {
                const command = urls[String(url)];
                if (!command) return nativeFetch(url, options);
                try {
                  const result = await handle(command, JSON.parse(options.body));
                  return new Response(JSON.stringify(result ?? null), {headers: {'Content-Type': 'application/json', 'Tauri-Response': 'ok'}});
                } catch (error) {
                  return new Response(JSON.stringify(String(error)), {headers: {'Content-Type': 'application/json', 'Tauri-Response': 'error'}});
                }
              };
              window.__fixtureReady = true;
            })();
            return true;
        """)
        try:
            for _ in range(50):
                if self.execute("return window.__fixtureReady;"):
                    break
                time.sleep(0.1)
            self.assertTrue(self.execute("return window.__fixtureReady;"))
            self.execute("document.querySelector('[aria-label=\"刷新\"]').click(); return true;")
            self.execute("[...document.querySelectorAll('[role=tab]')].find(b => b.textContent.trim() === '电源管理').click(); return true;")
            for _ in range(50):
                if 'Fixture fan' in self.execute("return document.body.innerText;"):
                    break
                time.sleep(0.1)
            self.assertIn('Fixture fan', self.execute("return document.body.innerText;"))
            self.execute("[...document.querySelectorAll('button')].find(b => b.textContent === '一键强冷').click(); return true;")
            for _ in range(50):
                if self.execute("return [...document.querySelectorAll('button')].some(b => b.textContent === '恢复自动' && !b.disabled);"):
                    break
                time.sleep(0.1)
            self.execute("[...document.querySelectorAll('button')].find(b => b.textContent === '恢复自动').click(); return true;")
            for _ in range(50):
                if self.execute("return [...document.querySelectorAll('button')].some(b => b.textContent === '一键强冷' && !b.disabled);"):
                    break
                time.sleep(0.1)
            self.execute("""
                const input = document.querySelector('.tuning-input-number');
                Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, '30.5');
                input.dispatchEvent(new Event('input', {bubbles: true}));
                return true;
            """)
            self.assertFalse(self.execute("return [...document.querySelectorAll('button')].find(b => b.textContent === '应用功耗').disabled;"))
            self.execute("[...document.querySelectorAll('button')].find(b => b.textContent === '应用功耗').click(); return true;")
            for _ in range(50):
                if '当前读回 30.5 W' in self.execute("return document.body.innerText;"):
                    break
                time.sleep(0.1)
            self.assertIn('当前读回 30.5 W', self.execute("return document.body.innerText;"))
            writes = self.execute("return window.__fixtureWrites;")
            self.assertEqual([w['command'] for w in writes], ['set_linux_fan_boost', 'set_linux_fan_boost', 'set_linux_power_limit'])
            self.assertEqual([writes[0]['args']['enabled'], writes[1]['args']['enabled']], [True, False])
            self.assertEqual(writes[2]['args']['watts'], 30.5)
        finally:
            self.execute("if (window.__nativeFetch) window.fetch = window.__nativeFetch; document.querySelector('[aria-label=\"刷新\"]').click(); return true;")
            self.execute("[...document.querySelectorAll('[role=tab]')].find(b => b.textContent.trim() === '状态总览').click(); return true;")

    def test_ui_oem_tuple_and_battery_gating(self):
        # Every OEM write is intercepted; this test never changes real hardware.
        self.execute("""
            window.__oemReady = false;
            (async () => {
              const snapshot = await window.__TAURI_INTERNALS__.invoke('get_linux_status');
              snapshot.model = 'OEM controls UI fixture';
              snapshot.ac_online = true;
              snapshot.oem = {available:true, writable:true, error:null, mode:1, custom_power:false,
                fan_boost:false, fan_rpm:[2500,2600], dc_jack:true,
                presets:[{pl1:35,pl2:35,pl4:70},{pl1:65,pl2:65,pl4:100},{pl1:90,pl2:90,pl4:100}],
                limits:{pl1:35,pl2:35,pl4:70}};
              window.__oemSnapshot = snapshot;
              window.__oemWrites = [];
              window.__nativeFetch = window.fetch;
              const commands = ['get_linux_status', ...window.__testHardwareCommands];
              const urls = Object.fromEntries(commands.map(c => [window.__TAURI_INTERNALS__.convertFileSrc(c,'ipc'),c]));
              window.fetch = async (url, options) => {
                const command = urls[String(url)];
                if (!command) return window.__nativeFetch(url,options);
                if (command === 'renew_linux_control_lease') return new Response('null',
                  {headers:{'Content-Type':'application/json','Tauri-Response':'ok'}});
                const args = JSON.parse(options.body);
                if (command !== 'get_linux_status') {
                  window.__oemWrites.push({command,args});
                  if (command === 'set_linux_oem_mode') snapshot.oem.mode = args.mode;
                  if (command === 'set_linux_oem_fan_boost') snapshot.oem.fan_boost = args.enabled;
                  if (command === 'set_linux_oem_cpu_limits') { snapshot.oem.limits = args; snapshot.oem.custom_power = true; }
                }
                return new Response(JSON.stringify(command === 'get_linux_status' ? structuredClone(snapshot) : null),
                  {headers:{'Content-Type':'application/json','Tauri-Response':'ok'}});
              };
              window.__oemReady = true;
            })(); return true;
        """)
        try:
            for _ in range(50):
                if self.execute('return window.__oemReady;'): break
                time.sleep(0.1)
            self.assertTrue(self.execute('return window.__oemReady;'))
            self.execute("document.querySelector('[aria-label=刷新]').click(); [...document.querySelectorAll('[role=tab]')].find(b => b.textContent.trim() === '电源管理').click(); return true;")
            self.wait_for_status_fixture('OEM controls UI fixture')
            for _ in range(50):
                if self.execute("return !!document.querySelector('.tuning-input-number:not(:disabled)');"): break
                time.sleep(0.1)
            def edit(index, number):
                self.execute(f"const e=document.querySelectorAll('.tuning-input-number')[{index}]; Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(e,'{number}'); e.dispatchEvent(new Event('input',{{bubbles:true}})); return true;")
            edit(1, 60.5)
            self.assertEqual(self.execute("return document.querySelectorAll('.tuning-input-number')[1].valueAsNumber;"), 60)
            self.assertTrue(self.execute("return [...document.querySelectorAll('button')].find(b=>b.textContent==='应用 CPU 功耗').disabled;"))
            edit(2, 80)
            self.assertFalse(self.execute("return [...document.querySelectorAll('button')].find(b=>b.textContent==='应用 CPU 功耗').disabled;"))
            self.execute("[...document.querySelectorAll('button')].find(b=>b.textContent==='应用 CPU 功耗').click(); return true;")
            for _ in range(50):
                if self.execute("return window.__oemWrites.length;"): break
                time.sleep(0.1)
            writes = self.execute('return window.__oemWrites;')
            self.assertEqual(writes, [{'command':'set_linux_oem_cpu_limits','args':{'pl1':35,'pl2':60,'pl4':80}}])
            # A recovery fault must block boost-off without issuing any IPC write.
            self.execute("window.__oemSnapshot.oem.fan_curve={supported:true,active:true,fault:true,applied:null}; window.__oemSnapshot.oem.fan_boost=true; document.querySelector('[aria-label=刷新]').click(); return true;")
            for _ in range(50):
                if self.execute("return [...document.querySelectorAll('button')].some(b=>b.textContent==='恢复自动' && b.disabled);"): break
                time.sleep(0.1)
            self.assertTrue(self.execute("return [...document.querySelectorAll('button')].find(b=>b.textContent==='恢复自动').disabled;"))
            self.assertIn('恢复成功前无法关闭强冷', self.execute('return document.body.innerText;'))
            self.execute("[...document.querySelectorAll('button')].find(b=>b.textContent==='恢复自动').click(); return true;")
            self.assertEqual(self.execute('return window.__oemWrites;'), writes)
            self.execute("window.__oemSnapshot.oem.fan_curve.active=false; window.__oemSnapshot.oem.fan_curve.fault=false; document.querySelector('[aria-label=刷新]').click(); return true;")
            for _ in range(50):
                if self.execute("return [...document.querySelectorAll('button')].some(b=>b.textContent==='恢复自动' && !b.disabled);"): break
                time.sleep(0.1)
            self.assertFalse(self.execute("return [...document.querySelectorAll('button')].find(b=>b.textContent==='恢复自动').disabled;"))
            self.execute("window.__oemSnapshot.ac_online=false; window.__oemSnapshot.oem.dc_jack=false; document.querySelector('[aria-label=刷新]').click(); return true;")
            for _ in range(50):
                if self.execute("return document.querySelector('.tuning-input-number').disabled;"): break
                time.sleep(0.1)
            self.assertTrue(self.execute("return document.querySelector('.tuning-input-number').disabled;"))
            self.assertEqual(self.execute("return [...document.querySelectorAll('.shared-mode-grid button')].map(b=>b.disabled);"), [False, True, True])
        finally:
            self.execute("if(window.__nativeFetch) window.fetch=window.__nativeFetch; document.querySelector('[aria-label=刷新]').click(); [...document.querySelectorAll('[role=tab]')].find(b=>b.textContent.trim()==='状态总览').click(); return true;")

    def test_native_curve_rejects_invalid_input(self):
        self.execute("""
          window.__curveError = null;
          window.__TAURI_INTERNALS__.invoke('set_linux_oem_fan_curve', {config: {
            cpu:[],gpu:[],isolated:true,ramp_enabled:true,ramp_ms:300}})
            .then(()=>window.__curveError='unexpected success').catch(e=>window.__curveError=String(e));
          return true;
        """)
        for _ in range(100):
            error = self.execute('return window.__curveError;')
            if error is not None:
                self.assertIn('曲线需要 8', error)
                return
            time.sleep(0.1)
        self.fail('Native curve validation timed out')

    def test_ui_fan_curve_draft_apply_restore(self):
        with self.isolated_config_document(), self.curve_fixture():
            previous_legacy = self.execute("return localStorage.getItem('openrevo_linux_fan_curve_v1');")
            self.execute("window.__curveFixture.ac_online=false; window.__curveFixture.oem.dc_jack=false; document.querySelector('[aria-label=刷新]').click(); return true;")
            self.navigate("电源管理")
            self.click_text("原版默认曲线")
            self.assertTrue(self.execute("return [...document.querySelectorAll('button')].find(b=>b.textContent==='应用风扇曲线').disabled;"))
            self.execute("document.querySelector('[aria-label=\"联动 CPU 和 GPU 曲线\"]').click(); document.querySelector('[aria-label=\"CPU 40°C 增加转速\"]').click(); return true;")
            self.click_text("保存曲线草稿")
            self.wait_js("document.body.innerText.includes('草稿已保存；启动时不会自动应用')")
            saved = json.loads(self.config_file.read_text())
            draft = saved['fan_curve_draft']
            self.assertEqual(saved['version'], 1)
            self.assertEqual(draft['cpu'][0]['duty'], 23)
            self.assertEqual(draft['cpu'], draft['gpu'])
            self.assertEqual(self.execute("return localStorage.getItem('openrevo_linux_fan_curve_v1');"), previous_legacy)
            self.assertEqual(self.execute('return window.__curveWrites;'), [])
            self.execute("window.__curveFixture.ac_online=true; window.__curveFixture.oem.dc_jack=true; document.querySelector('[aria-label=刷新]').click(); return true;")
            self.click_text("应用风扇曲线")
            self.wait_js("document.body.innerText.includes('上次应用已完成并读回确认')")
            self.assertEqual(self.execute('return window.__curveWrites;'),
                             [{'command': 'set_linux_oem_fan_curve', 'args': {'config': draft}}])
            self.click_text("恢复原风扇策略")
            self.wait_js("document.body.innerText.includes('已恢复自定义曲线启用前的风扇策略')")
            self.assertEqual(self.execute('return window.__curveWrites[1].command;'), 'restore_linux_oem_fan_curve')
            self.execute("document.querySelector('.linux-fan-curve-panel').scrollIntoView({block:'start'}); return true;")
            self.wait_js("document.querySelector('.linux-fan-curve-panel').getBoundingClientRect().top < 160")
            output = ROOT / 'test-results/linux-fan-curve.png'
            output.write_bytes(self.screenshot())
            # Capture the new settings controls as well as the editor above.
            self.execute("document.querySelector('.linux-fan-curve-panel > .sys-sub-panel').scrollIntoView({block:'center'}); return true;")
            time.sleep(0.2)
            (self.test_home / 'curve-settings.png').write_bytes(
                self.screenshot())

    def test_ui_legacy_curve_migration_preserves_source_without_applying(self):
        with self.isolated_config_document(), self.curve_fixture():
            previous = self.execute("return [localStorage.getItem('openrevo_linux_fan_curve_v1'), localStorage.getItem('openrevo_linux_fan_curve_v1_migrated')];")
            legacy = json.dumps(self.valid_draft(first_duty=27, ramp_ms=500))
            try:
                self.execute("localStorage.setItem('openrevo_linux_fan_curve_v1', arguments[0]); localStorage.removeItem('openrevo_linux_fan_curve_v1_migrated'); return true;", [legacy])
                self.navigate("电源管理")
                self.wait_js("document.body.innerText.includes('旧草稿已迁移到应用配置')")
                self.assertEqual(json.loads(self.config_file.read_text())['fan_curve_draft'], json.loads(legacy))
                self.assertEqual(self.execute("return localStorage.getItem('openrevo_linux_fan_curve_v1');"), legacy)
                self.assertEqual(self.execute("return localStorage.getItem('openrevo_linux_fan_curve_v1_migrated');"), 'true')
                self.assertEqual(self.execute('return window.__fixtureConfigWrites;'), 1)
                self.assertEqual(self.execute('return window.__curveWrites;'), [])
                reads = self.execute('return window.__fixtureConfigReads;')
                self.navigate("状态总览")
                self.navigate("电源管理")
                self.wait_js(f"window.__fixtureConfigReads > {reads} && !document.body.innerText.includes('正在读取草稿')")
                self.assertEqual(self.execute("return document.querySelector('[aria-label=\"CPU 40°C 转速\"]').getAttribute('aria-valuenow');"), '27')
                self.assertEqual(self.execute('return window.__fixtureConfigWrites;'), 1)
                self.assertEqual(self.execute('return window.__curveWrites;'), [])
                self.assertFalse((self.config_directory / 'config.json.bak').exists())
            finally:
                self.execute("['openrevo_linux_fan_curve_v1','openrevo_linux_fan_curve_v1_migrated'].forEach((key,index)=>{const value=arguments[0][index]; if(value===null)localStorage.removeItem(key);else localStorage.setItem(key,value);}); return true;", [previous])

    def test_native_configuration_validation_import_export_and_preservation(self):
        with self.isolated_config_document(), self.curve_fixture():
            self.assertEqual(self.native_ipc('get_linux_config'), {'ok': True, 'value': None})
            invalid = self.valid_draft()
            invalid['cpu'] = []
            result = self.native_ipc('save_linux_fan_curve_draft', {'draft': invalid})
            self.assertFalse(result['ok'])
            self.assertEqual(result['rejection']['code'], 'config_invalid')
            self.assertFalse(self.config_file.exists())
            result = self.native_ipc('import_linux_config', {'json': '{"version":2,"fan_curve_draft":null}'})
            self.assertFalse(result['ok'])
            self.assertEqual(result['rejection']['code'], 'config_version')
            self.assertFalse(self.config_file.exists())
            draft = self.valid_draft(first_duty=25)
            expected = {'version': 1, 'fan_curve_draft': draft}
            self.assertEqual(self.native_ipc('save_linux_fan_curve_draft', {'draft': draft}), {'ok': True, 'value': expected})
            export = self.native_ipc('export_linux_config')
            self.assertTrue(export['ok'])
            self.assertEqual(json.loads(export['value']), expected)
            imported = {'version': 1, 'fan_curve_draft': self.valid_draft(first_duty=28, ramp_ms=1000)}
            self.assertEqual(self.native_ipc('import_linux_config', {'json': json.dumps(imported)}), {'ok': True, 'value': imported})
            self.assertEqual(self.native_ipc('get_linux_config'), {'ok': True, 'value': imported})
            self.assertEqual(json.loads(self.config_file.read_text()), imported)
            backup = self.config_directory / 'config.json.bak'
            self.assertEqual(json.loads(backup.read_text()), expected)
            preserved_backup = backup.read_bytes()
            # Deliberately damaged/future files live only in this run's XDG home.
            for content, code in [(b'{"version":2,"future_setting":true}', 'config_version'),
                                  (b'{broken', 'config_invalid')]:
                self.config_file.write_bytes(content)
                for command, args in [('get_linux_config', {}),
                                      ('save_linux_fan_curve_draft', {'draft': draft}),
                                      ('import_linux_config', {'json': json.dumps(expected)})]:
                    result = self.native_ipc(command, args)
                    self.assertFalse(result['ok'])
                    self.assertEqual(result['rejection']['code'], code)
                    self.assertEqual(self.config_file.read_bytes(), content)
                    self.assertEqual(backup.read_bytes(), preserved_backup)
            self.assertEqual(self.execute('return window.__curveWrites;'), [])

    def test_ui_delayed_save_is_seen_after_navigation(self):
        with self.isolated_config_document(), self.curve_fixture():
            self.assertTrue(self.native_ipc('save_linux_fan_curve_draft', {'draft': self.valid_draft()})['ok'])
            self.navigate("电源管理")
            self.wait_js("!!document.querySelector('[aria-label=\"CPU 40°C 增加转速\"]:not(:disabled)')")
            self.execute("document.querySelector('[aria-label=\"CPU 40°C 增加转速\"]').click(); window.__delaySave=true; return true;")
            self.click_text("保存曲线草稿")
            self.wait_js("typeof window.__releaseSave === 'function'")
            reads = self.execute('return window.__fixtureConfigReads;')
            try:
                self.navigate("状态总览")
                self.navigate("电源管理")
                self.wait_js("document.body.innerText.includes('正在读取草稿')")
                self.assertEqual(self.execute('return window.__fixtureConfigReads;'), reads)
                self.assertEqual(json.loads(self.config_file.read_text())['fan_curve_draft']['cpu'][0]['duty'], 22)
            finally:
                self.execute("window.__delaySave=false; if(window.__releaseSave)window.__releaseSave(); window.__releaseSave=null; return true;")
            self.wait_js(f"window.__fixtureConfigReads > {reads} && !document.body.innerText.includes('正在读取草稿')")
            self.assertEqual(self.execute("return document.querySelector('[aria-label=\"CPU 40°C 转速\"]').getAttribute('aria-valuenow');"), '23')
            self.assertEqual(json.loads(self.config_file.read_text())['fan_curve_draft']['cpu'][0]['duty'], 23)
            self.assertEqual(self.execute('return window.__curveWrites;'), [])

    def test_ui_stale_status_stops_lease_and_allows_unconfirmed_restore(self):
        with self.isolated_config_document(), self.curve_fixture(active=True):
            self.navigate("电源管理")
            self.wait_js("[...document.querySelectorAll('button')].some(b=>b.textContent==='恢复原风扇策略' && !b.disabled)")
            self.wait_js("window.__fixtureRenewals > 0")
            self.execute("window.__statusHeld=true; document.querySelector('[aria-label=刷新]').click(); return true;")
            self.wait_js("window.__statusWaiters.length > 0")
            self.wait_js("document.querySelector('.linux-status').dataset.stale === 'true'", timeout=13)
            self.wait_js("document.body.innerText.includes('已停止续租')", timeout=4)
            renewals = self.execute('return window.__fixtureRenewals;')
            self.execute("window.__leaseStoppedAt=Date.now(); return true;")
            self.wait_js("Date.now()-window.__leaseStoppedAt > 2300", timeout=4)
            self.assertEqual(self.execute('return window.__fixtureRenewals;'), renewals)
            self.assertTrue(self.execute("return [...document.querySelectorAll('button')].find(b=>b.textContent==='应用风扇曲线').disabled;"))
            self.click_text("恢复原风扇策略")
            self.wait_js("window.__curveWrites.length === 1")
            self.assertEqual(self.execute('return window.__curveWrites[0].command;'), 'restore_linux_oem_fan_curve')
            self.wait_js("document.body.innerText.includes('未能确认当前状态')", timeout=7)
            self.assertFalse(self.execute("return [...document.querySelectorAll('button')].find(b=>b.textContent==='恢复原风扇策略').disabled;"))
            self.assertNotIn('已恢复自定义曲线启用前的风扇策略', self.execute('return document.body.innerText;'))
            self.execute("window.__statusHeld=false; for(const release of window.__statusWaiters.splice(0))release(); return true;")
            self.wait_js("document.querySelector('.linux-status').dataset.stale === 'false'")
            self.wait_js(f"window.__fixtureRenewals > {renewals}")
            self.assertTrue(self.execute("return [...document.querySelectorAll('button')].find(b=>b.textContent==='恢复原风扇策略').disabled;"))
            self.assertEqual(self.execute('return window.__curveWrites.length;'), 1)

    def test_refresh_and_capture(self):
        self.execute("document.querySelector('[aria-label=\"刷新\"]').click(); return true;")
        time.sleep(0.5)
        self.assertNotIn("连接中断", self.execute("return document.body.innerText;"))
        output = Path(os.environ.get("OPENREVO_TEST_SCREENSHOT", ROOT / "test-results/linux-window.png"))
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_bytes(self.screenshot())


if __name__ == "__main__":
    unittest.main(verbosity=2)
