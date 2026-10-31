"""Ensure the static hardware boundary gate fails for representative regressions."""
import importlib.util
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("vault_gate", ROOT / "scripts/check_vault_leaks.py")
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)
REGISTRY = json.loads((ROOT / "src-tauri/config/ec_registry.json").read_text())


class VaultGateTests(unittest.TestCase):
    def test_registered_literals_identifiers_and_comments_fail(self):
        for source in (
            "let address = 1873;", "let address = 0x0751u16;",
            "let cur_1873 = value;", "// EC register 1873",
            "let code = 0x9c40a488;", "wmi_read_ec(42);",
            "const ADDR_UNREGISTERED: u16 = 42;",
        ):
            with self.subTest(source=source):
                self.assertTrue(gate.scan_text(source, REGISTRY))

    def test_vault_references_and_unrelated_units_pass(self):
        for source in (
            "wmi_read_ec(ADDR_FAN_BOOST_CLAMP);",
            "thread::sleep(Duration::from_millis(2000));",
            "setTimeout(poll, 2000);", "gpu_vram_mb: 8192,",
        ):
            with self.subTest(source=source):
                self.assertEqual(gate.scan_text(source, REGISTRY), [])

    def test_timeout_exception_does_not_hide_another_address_on_the_line(self):
        self.assertTrue(gate.scan_text(
            "setTimeout(() => wmi_read_ec(1873), 2000);", REGISTRY
        ))


if __name__ == "__main__":
    unittest.main()
