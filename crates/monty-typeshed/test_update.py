"""Regression tests for native buffer stub transforms."""

import ast
import unittest

import update


class BufferAliasTests(unittest.TestCase):
    def test_all_aliases_are_narrowed(self):
        source = '\n'.join(f'{alias}: TypeAlias = Buffer' for alias in self.aliases())
        result = update.narrow_buffer_aliases(source)
        for alias in self.aliases():
            self.assertIn(f'{alias}: TypeAlias = bytes | bytearray | memoryview[Any]', result)

    def test_missing_upstream_alias_is_rejected(self):
        for missing in self.aliases():
            with self.subTest(alias=missing):
                source = '\n'.join(f'{alias}: TypeAlias = Buffer' for alias in self.aliases() if alias != missing)
                with self.assertRaisesRegex(ValueError, missing):
                    update.narrow_buffer_aliases(source)

    @staticmethod
    def aliases():
        return ['ReadOnlyBuffer', 'WriteableBuffer', 'ReadableBuffer']


class MemoryviewFormatTests(unittest.TestCase):
    def test_tolist_does_not_assume_a_shape(self):
        tree = ast.parse(
            'class memoryview:\n    def tolist(self) -> list[_I]: ...\n    def __getitem__(self, key: int) -> _I: ...'
        )
        update.update_memoryview_formats(tree)
        method = tree.body[0].body[0]
        self.assertEqual(ast.unparse(method.returns), 'Any')


if __name__ == '__main__':
    unittest.main()
