#!/usr/bin/env python3
"""Smoke test the actual Linux Tauri/WebKit window using W3C WebDriver.

Requires tauri-driver, WebKitWebDriver and a display (or xvfb-run).
Hardware mutations are intercepted by UI fixtures; native control tests send
only invalid requests. The suite never changes hardware or login preferences.
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
        cls.cli = Path(os.environ.get("OPENREVO_TEST_CLI", binary.parent / 'openrevo-cli')).resolve()
        if not cls.cli.is_file():
            raise RuntimeError("Build the CLI first: cargo build --manifest-path src-tauri/Cargo.toml -p openrevo-core --bin openrevo-cli --locked")
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
              'execute_linux_action', 'set_autostart_enabled', 'renew_linux_control_lease'
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
              snapshot.control_error = null;
              snapshot.domains = {
                performance: {active:'office', modes:['office','balanced','turbo','custom'].map(mode=>({mode,enabled:true,reason:null})),
                  limits:{pl1:35,pl2:35,pl4:70}, ceiling:{pl1:90,pl2:90,pl4:110}, limits_writable:true,
                  power_limits:[], error:null},
                devices: {battery:{supported:true,writable:true,mode:'balanced',lower:75,upper:80,error:null},switches:[{id:'fn_lock',enabled:false,writable:true,detail:'EC fixture'},{id:'camera',enabled:true,writable:true,detail:'EC fixture'}],fault:false,error:null},
                cooling: {rpm:[2500,2600],boosted:false,writable:true,error:null,curve_writable:true,
                  curve:{supported:true,active,fault:false,applied:active ? draft : null}}
              };
              snapshot.hotkeys_enabled=false;snapshot.hotkey_error=null;
              snapshot.wmi_events={available:true,last_code:null,last_sequence:null,dropped:0,error:null};
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
                const {action} = JSON.parse(options.body);
                if (command !== 'execute_linux_action') throw new Error('Unexpected write blocked by fixture');
                window.__curveWrites.push(action);
                if (action.kind === 'fan_curve') {
                  snapshot.domains.cooling.curve.active = true;
                  snapshot.domains.cooling.curve.applied = action.config;
                } else if (action.kind === 'restore_fan_curve') {
                  snapshot.domains.cooling.curve.active = false;
                  snapshot.domains.cooling.curve.applied = null;
                } else if (action.kind === 'mode') {
                  snapshot.domains.performance.active = action.mode;
                } else if (action.kind === 'fan') {
                  snapshot.domains.cooling.boosted = action.enabled;
                } else if (action.kind === 'battery') {
                  const limits={full:[95,100],balanced:[75,80],longevity:[55,60]}[action.policy.mode] ?? [action.policy.lower,action.policy.upper];
                  Object.assign(snapshot.domains.devices.battery,{mode:action.policy.mode,lower:limits[0],upper:limits[1]});
                } else if (action.kind === 'switch') {
                  snapshot.domains.devices.switches.find(s=>s.id===action.switch).enabled=action.enabled;
                } else if (action.kind === 'hotkeys') {
                  snapshot.hotkeys_enabled=action.enabled;
                } else if (action.kind === 'cpu_limits') {
                  snapshot.domains.performance.limits = action.limits;
                  snapshot.domains.performance.active = 'custom';
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
        self.assertEqual(self.execute("return document.querySelectorAll('.shared-mode-grid button').length;"), 4)

    def test_shared_navigation_and_power_page(self):
        self.assertTrue(self.execute("return !!document.querySelector('.shell-navbar .sub-nav-bar');"))
        self.assertEqual(self.execute("return document.querySelectorAll('.telemetry-cards-grid .telemetry-card').length;"), 4)
        self.execute("[...document.querySelectorAll('[role=tab]')].find(b => b.textContent.trim() === '电源管理').click(); return true;")
        text = self.execute("return document.body.innerText;")
        self.assertIn("性能释放模式", text)
        self.assertNotIn("命名场景", text)
        self.assertNotIn("此功能尚待移植", text)
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
        for action in [
            {'kind':'mode','mode':'invalid'},
            {'kind':'battery','policy':{'mode':'custom','lower':80,'upper':70}},
            {'kind':'switch','switch':'unknown','enabled':True},
            {'kind':'power_limit','id':'../../etc/passwd','watts':30},
            {'kind':'backlight','device':'../../etc/passwd','percent':50},
            {'kind':'fan','enabled':True,'address':42},
        ]:
            result = self.native_ipc('execute_linux_action', {'action':action})
            self.assertFalse(result['ok'], result)
        self.assertFalse(self.native_ipc('renew_linux_control_lease')['ok'])
        self.assertFalse(self.native_ipc('list_linux_scenes')['ok'])


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






    def test_native_curve_rejects_invalid_input(self):
        result = self.native_ipc('execute_linux_action', {'action': {'kind':'fan_curve', 'config': {
            'cpu':[], 'gpu':[], 'isolated':True, 'ramp_enabled':True, 'ramp_ms':300}}})
        self.assertFalse(result['ok'])
        self.assertIn('曲线需要 8', result['rejection']['message'])


    def test_ui_fan_curve_draft_apply_restore(self):
        with self.isolated_config_document(), self.curve_fixture():
            previous_legacy = self.execute("return localStorage.getItem('openrevo_linux_fan_curve_v1');")
            self.execute("window.__curveFixture.domains.cooling.curve_writable=false; document.querySelector('[aria-label=刷新]').click(); return true;")
            self.navigate("电源管理")
            self.click_text("原版默认曲线")
            self.assertTrue(self.execute("return [...document.querySelectorAll('button')].find(b=>b.textContent==='应用风扇曲线').disabled;"))
            self.execute("document.querySelector('[aria-label=\"联动 CPU 和 GPU 曲线\"]').click(); document.querySelector('[aria-label=\"CPU 40°C 增加转速\"]').click(); return true;")
            self.click_text("保存曲线草稿")
            self.wait_js("document.body.innerText.includes('草稿已保存；启动时不会自动应用')")
            saved = json.loads(self.config_file.read_text())
            draft = saved['fan_curve_draft']
            self.assertEqual(saved['version'], 3)
            self.assertEqual(draft['cpu'][0]['duty'], 23)
            self.assertEqual(draft['cpu'], draft['gpu'])
            self.assertEqual(self.execute("return localStorage.getItem('openrevo_linux_fan_curve_v1');"), previous_legacy)
            self.assertEqual(self.execute('return window.__curveWrites;'), [])
            self.execute("window.__curveFixture.domains.cooling.curve_writable=true; document.querySelector('[aria-label=刷新]').click(); return true;")
            self.click_text("应用风扇曲线")
            self.wait_js("document.body.innerText.includes('上次应用已完成并读回确认')")
            self.assertEqual(self.execute('return window.__curveWrites;'),
                             [{'kind':'fan_curve', 'config':draft}])
            self.click_text("恢复原风扇策略")
            self.wait_js("document.body.innerText.includes('已恢复自定义曲线启用前的风扇策略')")
            self.assertEqual(self.execute('return window.__curveWrites[1].kind;'), 'restore_fan_curve')
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
            result = self.native_ipc('import_linux_config', {'json': '{"version":99,"fan_curve_draft":null}'})
            self.assertFalse(result['ok'])
            self.assertEqual(result['rejection']['code'], 'config_version')
            self.assertFalse(self.config_file.exists())
            draft = self.valid_draft(first_duty=25)
            expected = {'version': 3, 'fan_curve_draft': draft, 'mode':None, 'custom_limits':None, 'hotkeys_enabled':False}
            self.assertEqual(self.native_ipc('save_linux_fan_curve_draft', {'draft': draft}), {'ok': True, 'value': expected})
            export = self.native_ipc('export_linux_config')
            self.assertTrue(export['ok'])
            self.assertEqual(json.loads(export['value']), expected)
            imported = {'version': 1, 'fan_curve_draft': self.valid_draft(first_duty=28, ramp_ms=1000)}
            migration = self.native_ipc('import_linux_config', {'json': json.dumps(imported)})
            imported.update(version=3, mode=None, custom_limits=None, hotkeys_enabled=False)
            self.assertEqual(migration, {'ok': True, 'value': imported})
            self.assertEqual(self.native_ipc('get_linux_config'), {'ok': True, 'value': imported})
            self.assertEqual(json.loads(self.config_file.read_text()), imported)
            backup = self.config_directory / 'config.json.bak'
            self.assertEqual(json.loads(backup.read_text()), expected)
            preserved_backup = backup.read_bytes()
            # Deliberately damaged/future files live only in this run's XDG home.
            for content, code in [(b'{"version":99,"future_setting":true}', 'config_version'),
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

    def test_native_cli_shares_runtime_and_rejects_invalid_actions(self):
        def cli(*args, code=0, environment=None):
            result = subprocess.run([str(self.cli), '--json', *args], text=True,
                                    capture_output=True, timeout=25, env=environment)
            self.assertEqual(result.returncode, code, result.stdout + result.stderr)
            return json.loads(result.stdout)
        with self.isolated_config_document():
            self.assertIsNone(cli('config')['result'])
            status = cli('status')['result']
            self.assertEqual(len(status['domains']['performance']['modes']), 4)
            self.assertFalse(cli('mode', 'invalid', code=2)['ok'])
            self.assertFalse(cli('scene', 'list', code=2)['ok'])
            self.assertFalse(cli('battery', '../../etc/passwd', '80', code=1)['ok'])
            self.assertFalse(self.config_file.exists())
            environment = dict(os.environ, DBUS_SESSION_BUS_ADDRESS='unix:path=/nonexistent-openrevo-test-bus')
            self.assertFalse(cli('status', code=3, environment=environment)['ok'])

    def test_unified_modes_and_fans_use_domain_actions(self):
        with self.curve_fixture():
            self.navigate('状态总览')
            self.wait_for_status_fixture('Fan curve UI fixture')
            self.execute("document.querySelector('.mode-custom').click(); return true;")
            self.wait_js("document.querySelector('.mode-custom').getAttribute('aria-pressed') === 'true'")
            self.click_text('一键强冷')
            self.click_text('恢复自动')
            self.wait_js("window.__curveWrites.length === 3")
            self.assertEqual(self.execute('return window.__curveWrites;'), [
                {'kind':'mode','mode':'custom'}, {'kind':'fan','enabled':True}, {'kind':'fan','enabled':False}])
            self.assertEqual(self.execute('return window.__fixtureRenewals;'), 0)
            self.assertNotIn('插件', self.execute("return [...document.querySelectorAll('[role=tab]')].map(t=>t.textContent).join(',');"))
            self.assertNotIn('水冷魔盒', self.execute("return [...document.querySelectorAll('[role=tab]')].map(t=>t.textContent).join(',');"))


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


    def test_ui_stale_status_has_no_lease_and_allows_unconfirmed_restore(self):
        with self.isolated_config_document(), self.curve_fixture(active=True):
            self.navigate("电源管理")
            self.wait_js("[...document.querySelectorAll('button')].some(b=>b.textContent==='恢复原风扇策略' && !b.disabled)")
            self.assertEqual(self.execute('return window.__fixtureRenewals;'), 0)
            self.execute("window.__statusHeld=true; document.querySelector('[aria-label=刷新]').click(); return true;")
            self.wait_js("window.__statusWaiters.length > 0")
            self.wait_js("document.querySelector('.linux-status').dataset.stale === 'true'", timeout=13)
            renewals = self.execute('return window.__fixtureRenewals;')
            self.execute("window.__leaseStoppedAt=Date.now(); return true;")
            self.wait_js("Date.now()-window.__leaseStoppedAt > 2300", timeout=4)
            self.assertEqual(self.execute('return window.__fixtureRenewals;'), renewals)
            self.assertTrue(self.execute("return [...document.querySelectorAll('button')].find(b=>b.textContent==='应用风扇曲线').disabled;"))
            self.click_text("恢复原风扇策略")
            self.wait_js("window.__curveWrites.length === 1")
            self.assertEqual(self.execute('return window.__curveWrites[0].kind;'), 'restore_fan_curve')
            self.wait_js("document.body.innerText.includes('未能确认当前状态')", timeout=7)
            self.assertFalse(self.execute("return [...document.querySelectorAll('button')].find(b=>b.textContent==='恢复原风扇策略').disabled;"))
            self.assertNotIn('已恢复自定义曲线启用前的风扇策略', self.execute('return document.body.innerText;'))
            self.execute("window.__statusHeld=false; for(const release of window.__statusWaiters.splice(0))release(); return true;")
            self.wait_js("document.querySelector('.linux-status').dataset.stale === 'false'")
            self.assertEqual(self.execute('return window.__fixtureRenewals;'), 0)
            self.assertTrue(self.execute("return [...document.querySelectorAll('button')].find(b=>b.textContent==='恢复原风扇策略').disabled;"))
            self.assertEqual(self.execute('return window.__curveWrites.length;'), 1)

    def test_window_close_keeps_runtime_available(self):
        # CloseRequested must hide the main window rather than end the owner.
        try:
            self.assertTrue(self.native_ipc('plugin:window|close', {'label':'main'})['ok'])
            self.wait_js("document.visibilityState === 'hidden'")
            result = subprocess.run([str(self.cli), '--json', 'config'], text=True, capture_output=True, timeout=25)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertTrue(json.loads(result.stdout)['ok'])
        finally:
            self.native_ipc('plugin:window|show', {'label':'main'})
        self.wait_js("document.visibilityState === 'visible'")

    def test_custom_cpu_controls_send_a_complete_tuple(self):
        with self.curve_fixture():
            self.navigate('电源管理')
            self.wait_js("document.querySelectorAll('.tuning-input-number').length === 3")
            self.execute("""
                const inputs = [...document.querySelectorAll('.tuning-input-number')];
                for (const [index, value] of [[1,'60'],[2,'80']]) {
                    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(inputs[index],value);
                    inputs[index].dispatchEvent(new Event('input',{bubbles:true}));
                }
                return true;
            """)
            self.click_text('应用 CPU 功耗')
            self.wait_js("window.__curveWrites.length === 1")
            self.assertEqual(self.execute('return window.__curveWrites[0];'),
                             {'kind':'cpu_limits','limits':{'pl1':35,'pl2':60,'pl4':80}})
            self.execute("window.__curveFixture.domains.performance.limits_writable=false; document.querySelector('[aria-label=刷新]').click(); return true;")
            self.wait_js("document.querySelector('.tuning-input-number').disabled")

    def test_ec_battery_presets_and_custom_thresholds(self):
        with self.curve_fixture():
            self.navigate('状态总览')
            self.click_text('长效 60%')
            self.wait_js("window.__curveWrites.length===1")
            self.assertEqual(self.execute('return window.__curveWrites[0];'), {'kind':'battery','policy':{'mode':'longevity','lower':None,'upper':None}})
            self.wait_js("document.querySelector('[aria-label=\"EC 充电下限\"]').value==='55'")
            self.execute("""
              for(const [label,value] of [['EC 充电下限','90'],['EC 充电上限','80']]) {
                const input=document.querySelector(`[aria-label="${label}"]`);
                Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(input,value);
                input.dispatchEvent(new Event('input',{bubbles:true}));
              } return true;
            """)
            self.assertTrue(self.execute("return [...document.querySelectorAll('button')].find(b=>b.textContent==='应用阈值').disabled;"))
            self.execute("""const input=document.querySelector('[aria-label="EC 充电下限"]');
              Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(input,'70');input.dispatchEvent(new Event('input',{bubbles:true}));return true;""")
            self.click_text('应用阈值')
            self.wait_js("window.__curveWrites.length===2")
            self.assertEqual(self.execute('return window.__curveWrites[1];'), {'kind':'battery','policy':{'mode':'custom','lower':70,'upper':80}})
            self.execute("document.querySelector('[aria-label=\"EC 充电下限\"]').scrollIntoView({block:'center'});return true;")
            (self.test_home / 'ec-battery.png').write_bytes(self.screenshot())

    def test_hardware_switch_and_key_linkage_use_semantic_actions(self):
        with self.curve_fixture():
            self.navigate('状态总览')
            self.execute("document.querySelector('[aria-label=\"Fn 锁定\"]').click();return true;")
            self.wait_js("window.__curveWrites.length===1")
            self.assertEqual(self.execute('return window.__curveWrites[0];'), {'kind':'switch','switch':'fn_lock','enabled':True})
            self.wait_js("!document.querySelector('[aria-label=\"机身模式键联动\"]').disabled")
            self.execute("document.querySelector('[aria-label=\"机身模式键联动\"]').click();return true;")
            self.wait_js("window.__curveWrites.length===2")
            self.assertEqual(self.execute('return window.__curveWrites[1];'), {'kind':'hotkeys','enabled':True})
            self.execute("window.__curveFixture.domains.devices.switches[0].writable=false;document.querySelector('[aria-label=刷新]').click();return true;")
            self.wait_js("document.querySelector('[aria-label=\"Fn 锁定\"]').disabled")
            self.execute("document.querySelector('[aria-label=\"机身模式键联动\"]').scrollIntoView({block:'center'});return true;")
            (self.test_home / 'hardware-switches.png').write_bytes(self.screenshot())

    def test_refresh_and_capture(self):
        self.execute("document.querySelector('[aria-label=\"刷新\"]').click(); return true;")
        time.sleep(0.5)
        self.assertNotIn("连接中断", self.execute("return document.body.innerText;"))
        output = Path(os.environ.get("OPENREVO_TEST_SCREENSHOT", ROOT / "test-results/linux-window.png"))
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_bytes(self.screenshot())


if __name__ == "__main__":
    unittest.main(verbosity=2)
