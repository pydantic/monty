"""Execute the same benchmark fixture as the Rust integration harness."""

import ast
import json
import os
from pathlib import Path

name = os.environ['MONTY_RSS_CASE']
if name != 'empty':
    directory = Path(os.environ['MONTY_RSS_FIXTURES'])
    entry = next(e for e in json.loads((directory / 'manifest.json').read_text()) if e['name'] == name)
    source = ast.parse((directory / f'{name}.py').read_text())
    last = source.body[-1]
    source.body[-1] = ast.Assign(targets=[ast.Name(id='benchmark_result', ctx=ast.Store())], value=last.value)
    namespace = {'data': (directory / f'{name}.input').read_text()}
    exec(compile(ast.fix_missing_locations(source), 'chunking.py', 'exec'), namespace)
    assert namespace['benchmark_result'] == entry['expected']
