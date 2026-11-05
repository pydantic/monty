"""Generate multilingual Markdown and validate both chunkers' complete output."""

import argparse
import ast
import hashlib
import json
from pathlib import Path

HERE = Path(__file__).resolve().parent
DIGEST = """\nchunks = chunk_document(data)
total = len(chunks) * 1000003
for start, chunk in chunks:
    total += start + len(chunk) + ord(chunk[0]) + ord(chunk[-1])
total
"""


def prepare(output):
    fixtures = output / 'fixtures'
    hot = output / 'hot'
    fixtures.mkdir(parents=True, exist_ok=True)
    hot.mkdir(exist_ok=True)
    corpus = ''.join(
        f'## Abschnitt {i}: API migration / 運用メモ 🧭\n\n'
        'The retrieval agent checks the service history before preparing an answer. '
        'It preserves the source location so every chunk can be traced to the original document.\n\n'
        'Für die nächste Veröffentlichung prüfen wir Größe, Laufzeit und Rückmeldungen. '
        'Änderungen an der Schnittstelle müssen erklärt werden; überraschende Abbrüche werden protokolliert.\n\n'
        '検索システムは文章を段落ごとに分割します。前後の文脈を残すため、隣り合う部分には重複があります。'
        '引用した箇所と元の資料を対応させ、回答を検証できるようにします。\n\n'
        f'Checkpoint {i:06d}: résumé, café, naïve indexing, 東京, München, and deployment notes 🔎.\n\n'
        for i in range(8000)
    ).encode()
    entries = []
    counts = []
    for kib, iterations in [(256, 3), (1024, 2), (2048, 1)]:
        data = corpus[: kib * 1024].decode(errors='ignore')
        outputs = []
        for variant in ['remainder', 'offsets']:
            source = (HERE / f'{variant}.py').read_text()
            namespace = {}
            exec(source, namespace)
            chunks = namespace['chunk_document'](data)
            outputs.append(chunks)
            assert chunks[0][0] == 0
            assert chunks[-1][0] + len(chunks[-1][1]) == len(data)
            for i, (start, chunk) in enumerate(chunks):
                assert 0 < len(chunk) <= 2000
                assert chunk == data[start : start + len(chunk)]
                if i:
                    previous, previous_chunk = chunks[i - 1]
                    assert start == previous + len(previous_chunk) - 256
            name = f'{variant}_{kib}kib'
            code = source + DIGEST
            expected = len(chunks) * 1000003 + sum(
                start + len(chunk) + ord(chunk[0]) + ord(chunk[-1]) for start, chunk in chunks
            )
            (fixtures / f'{name}.py').write_text(code)
            (fixtures / f'{name}.input').write_text(data)
            (fixtures / f'{name}.full.py').write_text(source + '\nchunk_document(data)\n')
            (fixtures / f'{name}.expected.json').write_text(json.dumps(chunks, ensure_ascii=False))
            tree = ast.parse(code)
            body = tree.body[:-1] + [ast.Return(tree.body[-1].value)]
            function = ast.FunctionDef(
                name='workload',
                args=ast.arguments(
                    posonlyargs=[], args=[ast.arg(arg='data')], kwonlyargs=[], kw_defaults=[], defaults=[]
                ),
                body=body,
                decorator_list=[],
            )
            loop = ast.parse(
                f'for iteration in range({iterations}):\n    result = workload(data)\n    assert result == {expected}'
            ).body
            module = ast.fix_missing_locations(
                ast.Module(body=[function, *loop, ast.Expr(ast.Name(id='result', ctx=ast.Load()))], type_ignores=[])
            )
            (hot / f'{name}.py').write_text(ast.unparse(module) + '\n')
            (hot / f'{name}.input').write_text(data)
            entries.append(
                dict(
                    name=name,
                    expected=expected,
                    input_bytes=len(data.encode()),
                    characters=len(data),
                    chunks=len(chunks),
                    variant=variant,
                    iterations=iterations,
                    retain=False,
                    input_sha256=hashlib.sha256(data.encode()).hexdigest(),
                )
            )
        assert outputs[0] == outputs[1]
        counts.append((kib, len(data), len(outputs[0])))
    for text in ['', 'hello', 'é中🐍' * 1200, 'a' * 2001, '\n\n' * 1800]:
        variants = []
        for variant in ['remainder', 'offsets']:
            namespace = {}
            exec((HERE / f'{variant}.py').read_text(), namespace)
            variants.append(namespace['chunk_document'](text))
        assert variants[0] == variants[1]
    (fixtures / 'manifest.json').write_text(json.dumps(entries, indent=2) + '\n')
    (hot / 'manifest.json').write_text(json.dumps(entries, indent=2) + '\n')
    print('CPython full chunk text/offset checks passed:', counts)
    return entries


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=Path('/private/tmp/monty-agent-chunking'))
    prepare(parser.parse_args().output)
