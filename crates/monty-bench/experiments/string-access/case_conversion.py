"""Compare bulk ASCII runs with per-character Unicode case conversion."""

import argparse
import hashlib
import json
import os
import subprocess
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--runtime', action='append', required=True, help='LABEL=/absolute/path/to/executable')
parser.add_argument('--circus', type=Path, required=True, help='Flying Circus checkout')
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--samples', type=int, default=30)
args = parser.parse_args()
root = Path(__file__).resolve().parents[4]
folder = args.output.resolve()
folder.mkdir(parents=True, exist_ok=False)
fixtures = folder / 'fixtures'
fixtures.mkdir()
paths = []
corpora = {
    'ascii': 'The Agent checks the Request ID before returning the Source URL. ',
    'german': 'Bitte API-Anfragen zur Straße prüfen und anschließend die Verbindung zum Gateway überprüfen.\n',
    'chinese': '中文代理请求处理结果检查来源文档',
    'chinese_mixed': '代理检查请求并查找相关文档，然后返回结果和来源。 request_id=42 status=open\n',
    'rare_emoji': 'The Agent checks the Request ID before returning the Source URL. ' * 100 + '🐍',
}
for corpus, phrase in corpora.items():
    text = (phrase * (200_000 // len(phrase) + 1))[:200_000]
    for method in ('lower', 'upper', 'casefold'):
        expected = getattr(text, method)()
        path = fixtures / f'case_{corpus}_{method}.py'
        path.write_text(
            f'TEXT = {text!r}\nEXPECTED = {expected!r}\n# --- benchmark ---\nfor _ in range(20):\n    result = TEXT.{method}()\nassert result == EXPECTED\n',
            encoding='utf-8',
        )
        paths.append(path)
for method in ('upper', 'casefold'):
    paths.append(root / f'crates/monty-bench/benches/python/str_{method}_mixed.py')
command = [str(args.circus / '.venv/bin/python'), '-m', 'flying_circus']
for runtime in args.runtime:
    command += ['-r', runtime]
command += [
    '--diff',
    '--threshold',
    '1',
    '--samples',
    str(args.samples),
    '--memory-samples',
    '0',
    '--startup-samples',
    '3',
    '--warmups',
    '3',
    '--output',
    str(folder / 'results'),
    '--format',
    'html',
    '--format',
    'markdown',
    '--format',
    'json',
]
for path in paths:
    command += ['--benchmark', str(path)]
(folder / 'provenance.json').write_text(
    json.dumps(
        {
            'command': command,
            'binaries': {
                label: hashlib.sha256(Path(binary).read_bytes()).hexdigest()
                for label, binary in (runtime.split('=', 1) for runtime in args.runtime)
            },
        },
        indent=2,
    )
    + '\n'
)
environment = dict(os.environ, PYTHONPATH=str(args.circus / 'src'), PYTHONDONTWRITEBYTECODE='1')
raise SystemExit(subprocess.run(command, env=environment).returncode)
