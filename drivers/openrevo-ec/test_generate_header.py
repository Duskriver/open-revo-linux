#!/usr/bin/env python3
"""Hardware-free checks for the kernel's registry boundary."""
import copy
import importlib.util
import json
import os
from pathlib import Path
import unittest

SOURCE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location('generate_header', SOURCE / 'generate_header.py')
GENERATOR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GENERATOR)
REGISTRY_PATH = Path(os.environ.get('OPENREVO_EC_REGISTRY', SOURCE.parents[1] / 'src-tauri/config/ec_registry.json'))
REGISTRY = json.loads(REGISTRY_PATH.read_text())


class HeaderTests(unittest.TestCase):
    def test_valid_registry_only_exports_read_whitelist(self):
        header = GENERATOR.generate(REGISTRY)
        self.assertIn('linux_ec_read[]', header)
        self.assertNotIn('linux_ec_write[]', header)
        self.assertIn('OR_CONTROL_FRAME_SIZE', header)

    def test_bad_integer_and_identifier_never_generate_c(self):
        for value in (None, True, -1, 0, 65536, 1.5, '1'):
            data = copy.deepcopy(REGISTRY)
            data['ec']['ADDR_CUSTOM_PL1'] = value
            with self.subTest(value=value), self.assertRaises(ValueError):
                GENERATOR.generate(data)
        data = copy.deepcopy(REGISTRY)
        data['ec']['BAD\n#error injected'] = 1
        with self.assertRaises(ValueError):
            GENERATOR.generate(data)

    def test_wire_layout_actions_and_power_bounds_are_checked(self):
        for name, value in [('FRAME_SIZE', 7), ('ARGUMENT_OFFSET', 7),
                            ('RESERVED_OFFSET', 4), ('MIN_POWER', 0),
                            ('PL4_MARGIN', 0), ('POWER', REGISTRY['linux_control']['MODE']),
                            ('REQUEST_OFFSET', REGISTRY['linux_fan']['REQUEST_OFFSET']),
                            ('REQUEST_OFFSET', REGISTRY['ec']['ADDR_CUSTOM_PL1'])]:
            data = copy.deepcopy(REGISTRY)
            data['linux_control'][name] = value
            with self.subTest(name=name, value=value), self.assertRaises(ValueError):
                GENERATOR.generate(data)

    def test_raw_writes_and_unknown_registers_are_rejected(self):
        data = copy.deepcopy(REGISTRY)
        data['linux_ec_write'] = ['ADDR_CUSTOM_PL1']
        with self.assertRaises(ValueError):
            GENERATOR.generate(data)
        data = copy.deepcopy(REGISTRY)
        data['linux_ec_read'].append('ADDR_MISSING')
        with self.assertRaises(ValueError):
            GENERATOR.generate(data)

    def test_mode_status_consumption_states_cannot_be_redefined(self):
        for name in ('MODE_STATUS_IDLE', 'MODE_STATUS_REQUEST'):
            for value in (None, 2, 256):
                data = copy.deepcopy(REGISTRY)
                if value is None:
                    del data['linux_control'][name]
                else:
                    data['linux_control'][name] = value
                with self.subTest(name=name, value=value), self.assertRaises(ValueError):
                    GENERATOR.generate(data)

    def test_wmi_fields_cannot_overlap_or_truncate_operation_codes(self):
        for name, value in [('DATA_OFFSET', 1), ('FUNCTION_OFFSET', 0),
                            ('READ', 256), ('WRITE', 256),
                            ('READ', REGISTRY['linux_wmi']['WRITE'])]:
            data = copy.deepcopy(REGISTRY)
            data['linux_wmi'][name] = value
            with self.subTest(name=name, value=value), self.assertRaises(ValueError):
                GENERATOR.generate(data)


if __name__ == '__main__':
    unittest.main()
