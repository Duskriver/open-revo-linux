#!/usr/bin/env python3
"""Smoke test the actual Linux Tauri/WebKit window using W3C WebDriver.

Requires tauri-driver, WebKitWebDriver and a display (or xvfb-run).
Does not change hardware settings or login startup preferences.
"""
import base64
from http.client import HTTPException
import json
import os
from pathlib import Path
import shutil
import subprocess
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
            raise RuntimeError("Install tauri-driver with cargo install tauri-driver --locked")
        binary = Path(os.environ.get("OPENREVO_TEST_BINARY", ROOT / "src-tauri/target/debug/open-revo")).resolve()
        if not binary.is_file():
            raise RuntimeError("Build the GUI first: pnpm tauri build --debug --no-bundle")
        environment = os.environ.copy()
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
    def execute(cls, script):
        return request("POST", f"/session/{cls.session}/execute/sync", {"script": script, "args": []})

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
        self.execute("document.querySelector('[aria-label=\"切换主题\"]').click(); return true;")
        time.sleep(0.15)
        self.assertNotEqual(self.execute("return document.documentElement.dataset.theme;"), initial)
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
              const commands = ['get_linux_status', 'set_linux_fan_boost', 'set_linux_power_limit', 'set_linux_platform_profile'];
              const urls = Object.fromEntries(commands.map(cmd => [window.__TAURI_INTERNALS__.convertFileSrc(cmd, 'ipc'), cmd]));
              const handle = async (command, args) => {
                if (command === 'get_linux_status') return structuredClone(snapshot);
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
              snapshot.ac_online = true;
              snapshot.oem = {available:true, writable:true, error:null, mode:1, custom_power:false,
                fan_boost:false, fan_rpm:[2500,2600], dc_jack:true,
                presets:[{pl1:35,pl2:35,pl4:70},{pl1:65,pl2:65,pl4:100},{pl1:90,pl2:90,pl4:100}],
                limits:{pl1:35,pl2:35,pl4:70}};
              window.__oemSnapshot = snapshot;
              window.__oemWrites = [];
              window.__nativeFetch = window.fetch;
              const commands = ['get_linux_status','set_linux_oem_mode','set_linux_oem_fan_boost','set_linux_oem_cpu_limits'];
              const urls = Object.fromEntries(commands.map(c => [window.__TAURI_INTERNALS__.convertFileSrc(c,'ipc'),c]));
              window.fetch = async (url, options) => {
                const command = urls[String(url)];
                if (!command) return window.__nativeFetch(url,options);
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
        self.execute("""
          window.__curveReady=false;
          window.__previousCurveDraft=localStorage.getItem('openrevo_linux_fan_curve_v1');
          localStorage.removeItem('openrevo_linux_fan_curve_v1');
          (async()=>{
            const snapshot=await window.__TAURI_INTERNALS__.invoke('get_linux_status');
            snapshot.ac_online=false; snapshot.oem.available=true; snapshot.oem.writable=true;
            snapshot.oem.dc_jack=false; snapshot.oem.fan_boost=false;
            snapshot.oem.fan_curve={supported:true, active:false, fault:false, applied:null};
            window.__curveFixture=snapshot; window.__curveWrites=[];
            window.__nativeFetch=window.fetch;
            const cmds=['get_linux_status','set_linux_oem_fan_curve','restore_linux_oem_fan_curve'];
            const urls=Object.fromEntries(cmds.map(c=>[window.__TAURI_INTERNALS__.convertFileSrc(c,'ipc'),c]));
            window.fetch=async(url,options)=>{
              const command=urls[String(url)]; if(!command) return window.__nativeFetch(url,options);
              const args=JSON.parse(options.body);
              if(command!=='get_linux_status'){
                window.__curveWrites.push({command,args});
                snapshot.oem.fan_curve.active=command==='set_linux_oem_fan_curve';
                snapshot.oem.fan_curve.applied=command==='set_linux_oem_fan_curve'?args.config:null;
              }
              return new Response(JSON.stringify(command==='get_linux_status'?structuredClone(snapshot):null),
                {headers:{'Content-Type':'application/json','Tauri-Response':'ok'}});
            };
            window.__curveReady=true;
          })(); return true;
        """)
        try:
            for _ in range(100):
                if self.execute('return window.__curveReady;'): break
                time.sleep(0.1)
            self.assertTrue(self.execute('return window.__curveReady;'))
            self.execute("document.querySelector('[aria-label=刷新]').click(); [...document.querySelectorAll('[role=tab]')].find(b=>b.textContent.trim()==='电源管理').click(); return true;")
            for _ in range(100):
                if self.execute("return !!document.querySelector('.shared-fan-editor');"): break
                time.sleep(0.1)
            self.execute("[...document.querySelectorAll('button')].find(b=>b.textContent==='原版默认曲线').click(); return true;")
            self.assertTrue(self.execute("return [...document.querySelectorAll('button')].find(b=>b.textContent==='应用风扇曲线').disabled;"))
            self.execute("document.querySelector('[aria-label=\"联动 CPU 和 GPU 曲线\"]').click(); document.querySelector('[aria-label=\"CPU 40°C 增加转速\"]').click(); return true;")
            self.execute("[...document.querySelectorAll('button')].find(b=>b.textContent==='保存曲线草稿').click(); return true;")
            draft=self.execute("return JSON.parse(localStorage.getItem('openrevo_linux_fan_curve_v1'));")
            self.assertEqual(draft['cpu'][0]['duty'],23)
            self.assertEqual(draft['cpu'],draft['gpu'])
            self.assertEqual(self.execute('return window.__curveWrites;'),[])
            self.execute("window.__curveFixture.ac_online=true; window.__curveFixture.oem.dc_jack=true; document.querySelector('[aria-label=刷新]').click(); return true;")
            for _ in range(100):
                if self.execute("return ![...document.querySelectorAll('button')].find(b=>b.textContent==='应用风扇曲线').disabled;"): break
                time.sleep(0.1)
            self.execute("[...document.querySelectorAll('button')].find(b=>b.textContent==='应用风扇曲线').click(); return true;")
            for _ in range(100):
                if self.execute("return ![...document.querySelectorAll('button')].find(b=>b.textContent==='恢复原风扇策略').disabled;"): break
                time.sleep(0.1)
            writes=self.execute('return window.__curveWrites;')
            self.assertEqual(writes,[{'command':'set_linux_oem_fan_curve','args':{'config':draft}}])
            self.execute("[...document.querySelectorAll('button')].find(b=>b.textContent==='恢复原风扇策略').click(); return true;")
            for _ in range(100):
                if self.execute('return window.__curveWrites.length;')==2: break
                time.sleep(0.1)
            self.assertEqual(self.execute('return window.__curveWrites[1].command;'),'restore_linux_oem_fan_curve')
            for _ in range(100):
                if self.execute("return [...document.querySelectorAll('button')].find(b=>b.textContent==='恢复原风扇策略').disabled && ![...document.querySelectorAll('button')].find(b=>b.textContent==='原版默认曲线').disabled;"): break
                time.sleep(0.1)
            self.execute("document.querySelector('.linux-fan-curve-panel').scrollIntoView({block:'start'}); return true;")
            for _ in range(50):
                if self.execute("return document.querySelector('.linux-fan-curve-panel').getBoundingClientRect().top < 120;"): break
                time.sleep(0.1)
            output=ROOT/'test-results/linux-fan-curve.png'
            output.write_bytes(base64.b64decode(request('GET',f'/session/{self.session}/screenshot')))
        finally:
            self.execute("if(window.__nativeFetch)window.fetch=window.__nativeFetch; if(window.__previousCurveDraft===null)localStorage.removeItem('openrevo_linux_fan_curve_v1'); else localStorage.setItem('openrevo_linux_fan_curve_v1',window.__previousCurveDraft); document.querySelector('[aria-label=刷新]').click(); [...document.querySelectorAll('[role=tab]')].find(b=>b.textContent.trim()==='状态总览').click(); return true;")

    def test_refresh_and_capture(self):
        self.execute("document.querySelector('[aria-label=\"刷新\"]').click(); return true;")
        time.sleep(0.5)
        self.assertNotIn("连接中断", self.execute("return document.body.innerText;"))
        output = Path(os.environ.get("OPENREVO_TEST_SCREENSHOT", ROOT / "test-results/linux-window.png"))
        output.parent.mkdir(parents=True, exist_ok=True)
        image = request("GET", f"/session/{self.session}/screenshot")
        output.write_bytes(base64.b64decode(image))


if __name__ == "__main__":
    unittest.main(verbosity=2)
