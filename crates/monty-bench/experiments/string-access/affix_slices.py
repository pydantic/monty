"""Measure affix checks and contiguous slices against saved interpreter builds."""

import argparse
import hashlib
import json
import os
import subprocess
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--runtime', action='append', required=True, help='LABEL=/absolute/path/to/executable')
parser.add_argument('--circus', type=Path, required=True, help='Flying Circus checkout with Unicode agent workloads')
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--samples', type=int, default=30)
parser.add_argument('--memory-samples', type=int, default=3)
args = parser.parse_args()
folder = args.output.resolve()
folder.mkdir(parents=True, exist_ok=False)
fixtures = folder / 'fixtures'
fixtures.mkdir()
paths = []
corpora = {
    'ascii': 'The agent checks the request and returns the source document. ',
    'german': 'Überprüfung der Anfrage: Der Agent sammelt Quellen und erstellt einen Bericht über die Ergebnisse. ',
    'chinese': '代理检查请求并查找相关文档，然后返回结果和来源。 request_id=42 status=open\n',
}
for corpus, phrase in corpora.items():
    body = (phrase * (16384 // len(phrase) + 1))[:16384]
    setup = f"TEXT = {body!r}\ntext = ''.join([TEXT, ''])\n# --- benchmark ---\n"
    base_setup = setup
    cases = {
        'startswith': (f'text.startswith({body[:16]!r})', True),
        'endswith': (f'text.endswith({body[-16:]!r})', True),
        'startswith_miss': ("text.startswith('not-present-XYZ')", False),
        'endswith_miss': ("text.endswith('not-present-XYZ')", False),
        'startswith_bounded': (f'text.startswith({body[8192:8208]!r}, 8192, 8256)', True),
        'endswith_bounded': (f'text.endswith({body[8240:8256]!r}, 8192, 8256)', True),
        'startswith_tuple': (f"text.startswith(('not-present-XYZ', {body[:16]!r}))", True),
    }
    for name, (expression, expected) in cases.items():
        path = fixtures / f'affix_{corpus}_{name}.py'
        path.write_text(setup + f'for _ in range(100):\n    result = {expression}\nassert result == {expected!r}\n')
        paths.append(path)
    for name, positions in {
        'random': [(i * 7919) % (len(body) - 64) for i in range(1024)],
        'nearby': [4096 + i for i in range(1024)],
    }.items():
        expected = sum(ord(body[position : position + 64][0]) for position in positions)
        path = fixtures / f'slice_{corpus}_{name}.py'
        setup = f"TEXT = {body!r}\ntext = ''.join([TEXT, ''])\nPOSITIONS = {positions!r}\n# --- benchmark ---\n"
        path.write_text(
            setup
            + f'total = 0\nfor position in POSITIONS:\n    excerpt = text[position:position + 64]\n    total += ord(excerpt[0])\nassert total == {expected}\n'
        )
        paths.append(path)
    path = fixtures / f'slice_{corpus}_whole.py'
    path.write_text(base_setup + 'for _ in range(100):\n    result = text[:]\nassert result == TEXT\n')
    paths.append(path)
for language in ('german_ascii', 'german', 'german_emoji', 'chinese'):
    paths.append(args.circus / 'src/flying_circus/workloads' / f'agent_unicode_{language}_large.py')
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
    str(args.memory_samples),
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
