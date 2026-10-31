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
        self.assertIn("系统性能档位", text)
        self.assertIn("硬件支持状态", text)
        self.assertNotIn("启动失败", text)
        self.assertEqual(self.execute("return !!document.querySelector('.linux-shell');"), True)
        self.assertEqual(self.execute("return document.querySelectorAll('.linux-profile-options button').length;"), 3)

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
                    self.assertTrue(self.execute("return [...document.querySelectorAll('.linux-profile-options button')].every(b => b.disabled);"))
                return
            time.sleep(0.1)
        self.fail("Profile write/readback did not complete")

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
