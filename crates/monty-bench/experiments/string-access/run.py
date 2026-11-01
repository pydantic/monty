"""Compare production string policies with Flying Circus, including cold access and RSS."""

import argparse
import fnmatch
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
parser.add_argument(
    '--only', action='append', default=[], help='Only workload stems matching these shell-style patterns'
)
parser.add_argument('--memory-samples', type=int, default=5)
args = parser.parse_args()
root = Path(__file__).resolve().parents[4]
folder = args.output.resolve()
folder.mkdir(parents=True, exist_ok=False)
fixtures = folder / 'fixtures'
fixtures.mkdir()
paths = []
corpora = {
    'ascii': 'The agent checks the request and returns the source document. ',
    'german': 'Die Anfrage wurde geprüft. Der Agent sammelt Quellen und erstellt einen Bericht über die Ergebnisse. ',
    'latin1_10pct': 'abcdefghiü',
    'chinese': '代理检查请求并查找相关文档，然后返回结果和来源。 request_id=42 status=open\n',
    'emoji_10pct': 'abcdefghi🐍',
}
for corpus, phrase in corpora.items():
    text = (phrase * (16384 // len(phrase) + 1))[:16384]
    for pattern, count in [('cold', 16), ('random', 4096), ('nearby', 4096), ('ends', 4096)]:
        positions = {
            'cold': [(i * 7919) % len(text) for i in range(count)],
            'random': [(i * 7919) % len(text) for i in range(count)],
            'nearby': [4096 + i // 2 + i % 2 for i in range(count)],
            'ends': [0 if i % 2 else -1 for i in range(count)],
        }[pattern]
        positions = [p - len(text) if i % 2 and p >= 0 else p for i, p in enumerate(positions)]
        expected = sum(ord(text[p]) for p in positions)
        path = fixtures / f'access_{corpus}_{pattern}.py'
        path.write_text(
            f'TEXT = {text!r}\nPOSITIONS = {positions!r}\n# --- benchmark ---\ntotal = 0\nfor position in POSITIONS:\n    total += ord(TEXT[position])\nassert total == {expected}\n'
        )
        paths.append(path)
    body = text[:4096]
    positions = [(i * 7919) % len(body) for i in range(64)]
    path = fixtures / f'memory_index_{corpus}.py'
    expected = 3000 * sum(ord(body[p]) for p in positions)
    path.write_text(
        f'BODY = {body!r}\nPOSITIONS = {positions!r}\n# --- benchmark ---\ntexts = [BODY + str(i) for i in range(3000)]\ntotal = 0\nfor text in texts:\n    for position in POSITIONS:\n        total += ord(text[position])\nassert total == {expected}\n'
    )
    paths.append(path)
for corpus in ('ascii', 'german', 'chinese'):
    phrase = corpora[corpus]
    body = (phrase * (16384 // len(phrase) + 1))[:16384]
    needle = 'INC-XYZ-17'
    text = ''.join(body[i : i + 1024] + needle for i in range(0, len(body), 1024))
    first = text.find(needle)
    cases = {
        'contains_hit': ('NEEDLE in TEXT', True),
        'find_hit': ('TEXT.find(NEEDLE)', first),
        'find_miss': ("TEXT.find('not-present-XYZZ')", -1),
        'find_bounded': (f'TEXT.find(NEEDLE, {first - 20}, {first + 30})', first),
        'rfind_hit': ('TEXT.rfind(NEEDLE)', text.rfind(needle)),
        'count': ('TEXT.count(NEEDLE)', 16),
        'count_empty': ("TEXT.count('')", len(text) + 1),
    }
    setup = f'TEXT = {text!r}\nNEEDLE = {needle!r}\n# --- benchmark ---\n'
    for name, (expression, expected) in cases.items():
        path = fixtures / f'search_{corpus}_{name}.py'
        path.write_text(setup + f'for _ in range(100):\n    value = {expression}\nassert value == {expected!r}\n')
        paths.append(path)
    path = fixtures / f'search_{corpus}_successive.py'
    path.write_text(
        setup
        + """for _ in range(10):
    position = 0
    count = 0
    while True:
        position = TEXT.find(NEEDLE, position)
        if position < 0:
            break
        count += 1
        position += len(NEEDLE)
assert count == 16
"""
    )
    paths.append(path)
for language in ('german_ascii', 'german', 'german_emoji', 'chinese'):
    paths.append(args.circus / 'src/flying_circus/workloads' / f'agent_unicode_{language}_large.py')
for name in (
    'string_random_access_ascii',
    'string_random_access_interned',
    'string_random_access_unicode',
    'string_end_access_unicode',
):
    paths.append(root / 'crates/monty-bench/benches/python' / f'{name}.py')
if args.only:
    paths = [path for path in paths if any(fnmatch.fnmatchcase(path.stem, pattern) for pattern in args.only)]
if not paths:
    parser.error('No workloads match --only')
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
