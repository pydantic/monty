"""Regression checks for benchmark input handling and reproducible summaries."""

import argparse
import io
import json
import runpy
import sys
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from tempfile import TemporaryDirectory
from types import ModuleType
from unittest.mock import patch

from benchmark_support import competing_processes, parse_variants, positive_seconds, wait_for_idle

TOOLS = Path(__file__).resolve().parents[1]


class BenchmarkToolsTests(unittest.TestCase):
    def test_variant_labels(self):
        parser = argparse.ArgumentParser()
        for specifications in [
            ['missing-separator'],
            ['x='],
            ['../x=worker'],
            ['x=a', 'x=b'],
            ['X=a', 'x=b'],
            ['cpython=worker'],
        ]:
            with self.subTest(specifications=specifications), redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as error:
                    parse_variants(parser, specifications)
                self.assertEqual(error.exception.code, 2)
        variants = parse_variants(parser, ['baseline=path with spaces/worker', 'candidate=other=worker'])
        self.assertEqual(variants['candidate'], str(Path('other=worker').resolve()))

    def test_contention_on_linux_and_macos(self):
        processes = (
            'PID %CPU COMM\n'
            '1 0 cargo\n'
            '2 0 /opt/rust/bin/rustc\n'
            '3 10 candidate-acces\n'
            '4 10 /opt/baseline-access\n'
            '5 0 llvm-objdump\n'
            '6 10 monty\n'
            '7 0 monty\n'
            '8 90 unrelated\n'
        )
        with patch('benchmark_support.subprocess.check_output', return_value=processes):
            self.assertEqual(competing_processes({6}), processes.splitlines()[1:6])

    def test_idle_wait_is_bounded(self):
        with patch('benchmark_support.time.monotonic', side_effect=[0, 2]):
            with self.assertRaisesRegex(TimeoutError, 'cargo'):
                wait_for_idle(lambda: ['cargo'], 1, 'sample')
        wait_for_idle(lambda: [], 1, 'sample')
        for value in ['0', '-1', 'nan', 'inf']:
            with self.subTest(value=value), self.assertRaises(argparse.ArgumentTypeError):
                positive_seconds(value)

    def test_moved_microbenchmark_runs(self):
        for script, header, rows, expected in [
            ('summarize_access.py', 'case,interpreter,ns', 'lookup,monty,1000000\n', 'median_ms'),
            ('summarize_nop.py', 'nops,interpreter,ns', '0,monty,10\n100,monty,110\n', 'median_ns_per_op'),
        ]:
            with self.subTest(script=script), TemporaryDirectory() as directory:
                run = Path(directory)
                (run / 'sample.csv').write_text(header + '\n' + rows)
                (run / 'accepted.json').write_text(
                    json.dumps([{'label': 'candidate', 'path': '/old/location/sample.csv'}])
                )
                output = io.StringIO()
                with patch.object(sys, 'argv', [script, str(run)]), redirect_stdout(output):
                    runpy.run_path(str(TOOLS / script), run_name='__main__')
                result = json.loads(output.getvalue())
                if script == 'summarize_access.py':
                    result = result['lookup']
                self.assertEqual(result['candidate'][expected], 1)

    def test_worker_summary_rejects_mixed_provenance(self):
        differences = ModuleType('flying_circus.differences')
        differences.difference = lambda *args, **kwargs: {'status': 'unchanged', 'change_percent': 0}
        modules = {'flying_circus': ModuleType('flying_circus'), 'flying_circus.differences': differences}
        for changed in [None, 'platform', 'harness_sha256', 'worker_adapter_sha256', 'warmups', 'source']:
            with self.subTest(changed=changed), TemporaryDirectory() as directory:
                root = Path(directory)
                runs = [root / 'first', root / 'second']
                for index, run in enumerate(runs):
                    run.mkdir()
                    metadata = {
                        'command': [],
                        'platform': 'same machine',
                        'harness_sha256': 'harness',
                        'worker_adapter_sha256': 'adapter',
                        'scope': 'repeated',
                        'contention': 'checked',
                        'warmups': 5,
                        'samples': 1,
                        'binaries': {'baseline': {'sha256': 'base'}, 'threaded': {'sha256': 'candidate'}},
                        'sources': {'lookup': {'sha256': 'source'}},
                    }
                    if index and changed == 'source':
                        metadata['sources']['lookup']['sha256'] = 'different'
                    elif index and changed:
                        metadata[changed] = 'different'
                    (run / 'metadata.json').write_text(json.dumps(metadata))
                    (run / 'samples.jsonl').write_text(
                        '\n'.join(
                            json.dumps({'workload': 'lookup', 'runtime': label, 'sample': 0, 'accepted': True, 'ns': 1})
                            for label in ['baseline', 'threaded']
                        )
                    )
                argv = [
                    'summarize_workers.py',
                    *map(str, runs),
                    '--flying-circus-source',
                    str(root),
                    '--output',
                    str(root / 'summary'),
                ]
                with (
                    patch.object(sys, 'argv', argv),
                    patch.object(sys, 'path', sys.path.copy()),
                    patch.dict(sys.modules, modules),
                    redirect_stdout(io.StringIO()),
                ):
                    if changed:
                        with self.assertRaisesRegex(ValueError, 'Mixed'):
                            runpy.run_path(str(TOOLS / argv[0]), run_name='__main__')
                    else:
                        runpy.run_path(str(TOOLS / argv[0]), run_name='__main__')
                        summary = json.loads((root / 'summary/summary.json').read_text())
                        self.assertEqual(summary['results']['lookup']['sample_count'], {'baseline': 2, 'threaded': 2})
