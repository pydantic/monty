"""Regression tests for the shared patch generator."""

import unittest
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import patch

from vendor_common import apply_patches, make_patch, write


class MakePatchTests(unittest.TestCase):
    def test_patch_name_cannot_escape_patch_dir(self) -> None:
        with TemporaryDirectory() as temp:
            root = Path(temp)
            for name in ('../outside', str(root / 'outside'), r'..\outside'):
                with self.subTest(name=name), self.assertRaisesRegex(SystemExit, 'without directory components'):
                    make_patch(name, 'reason', {}, root, root / 'patches')

    def test_patch_round_trips_non_lf_separators(self) -> None:
        with TemporaryDirectory() as temp:
            root = Path(temp)
            upstream = {'module.py': '# old\fform feed\u2028separator\n'}
            updated = {'module.py': '# new\fform feed\u2028separator\n'}
            source_dir = root / 'sources'
            patch_dir = root / 'patches'
            write(source_dir / 'module.py', updated['module.py'])
            with patch('vendor_common.REPO', root):
                make_patch('0001-module', 'reason', upstream, source_dir, patch_dir)
                result = apply_patches(upstream, [patch_dir / '0001-module.patch'])
            self.assertEqual(result, updated)
