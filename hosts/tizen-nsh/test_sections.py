"""Tests of sections.py on synthetic modules (no build needed)."""
import importlib.util
import pathlib
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("sections", pathlib.Path(__file__).with_name("sections.py"))
sections = importlib.util.module_from_spec(spec)
spec.loader.exec_module(sections)


def leb(value):
    out = bytearray()
    while True:
        byte = value & 0x7F
        value >>= 7
        out.append(byte | (0x80 if value else 0))
        if not value:
            return bytes(out)


def module(*parts):
    data = b"\0asm\x01\0\0\0"
    for kind, body in parts:
        data += bytes([kind]) + leb(len(body)) + body
    return data


class Sections(unittest.TestCase):
    def test_reads_code_data_and_custom_sections(self):
        name = b"name"
        data = module((1, b"\0"), (10, b"x" * 300), (11, b"y" * 20), (0, leb(len(name)) + name + b"z" * 5))
        sizes = sections.sections(data)
        self.assertEqual(sizes["code"], 300)
        self.assertEqual(sizes["data"], 20)
        self.assertEqual(sizes["custom:name"], len(name) + 1 + 5)

    def test_the_budget_fails_a_larger_code_section(self):
        with tempfile.NamedTemporaryFile(suffix=".wasm") as file:
            file.write(module((10, b"x" * 300)))
            file.flush()
            self.assertEqual(sections.main([file.name, "--max-code", "300"]), 0)
            self.assertEqual(sections.main([file.name, "--max-code", "299"]), 1)

    def test_refuses_what_is_not_a_module(self):
        with self.assertRaises(ValueError):
            sections.sections(b"not wasm")


if __name__ == "__main__":
    unittest.main()
