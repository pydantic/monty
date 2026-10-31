"""Build and run the standalone index-policy matrix and fresh-process RSS samples."""

import argparse
import csv
import hashlib
import json
import platform
import statistics
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path


def collect(command):
    import resource

    result = subprocess.run(command, capture_output=True, text=True, timeout=60)
    peak = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss
    print(
        json.dumps(
            {
                'returncode': result.returncode,
                'stdout': result.stdout,
                'stderr': result.stderr,
                'rss': peak if sys.platform == 'darwin' else peak * 1024,
            }
        )
    )


def memory(binary, output, samples):
    policies = (
        'scan',
        'cursor_back8',
        'sparse8_at8',
        'sparse32_at8',
        'full_at8',
        'deltas_at8',
        'adaptive8_16x',
        'adaptive32_16x',
    )
    with output.open('w') as stream:
        writer = csv.writer(stream, lineterminator='\n')
        writer.writerow(
            (
                'corpus',
                'requests_per_string',
                'policy',
                'text_bytes',
                'cache_bytes',
                'prototype_metadata_bytes',
                'strings',
                'rss_median_bytes',
                'rss_samples',
            )
        )
        for corpus in ('german', 'latin1_10pct', 'chinese'):
            for accesses in (1, 64):
                for policy in policies:
                    peaks = []
                    for _ in range(samples):
                        command = [
                            sys.executable,
                            str(Path(__file__).resolve()),
                            '--collect',
                            str(binary),
                            'memory',
                            policy,
                            corpus,
                            str(accesses),
                        ]
                        result = subprocess.run(command, capture_output=True, text=True, check=True)
                        sample = json.loads(result.stdout)
                        if sample['returncode']:
                            raise RuntimeError(sample['stderr'])
                        text, cache, metadata, strings, checksum = map(int, sample['stdout'].strip().split(','))
                        assert strings == 3000 and checksum > 0
                        peaks.append(sample['rss'])
                    writer.writerow(
                        (
                            corpus,
                            accesses,
                            policy,
                            text,
                            cache,
                            metadata,
                            strings,
                            statistics.median(peaks),
                            json.dumps(peaks),
                        )
                    )
                    stream.flush()
                print(f'RSS: {corpus}, {accesses} accesses/string', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True, help='New results directory')
    parser.add_argument('--rss-samples', type=int, default=3)
    parser.add_argument('--skip-matrix', action='store_true')
    args = parser.parse_args()
    if args.rss_samples < 0:
        parser.error('--rss-samples must be nonnegative')
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    source = Path(__file__).with_name('main.rs').resolve()
    binary = output / ('unicode-index.exe' if sys.platform == 'win32' else 'unicode-index')
    command = ['rustc', '--edition=2024', '-O', str(source), '-o', str(binary)]
    subprocess.run(command, check=True)
    subprocess.run([str(binary), 'validate'], check=True)
    if not args.skip_matrix:
        with (output / 'matrix.csv').open('w') as stream:
            subprocess.run([str(binary)], stdout=stream, check=True)
    if args.rss_samples and sys.platform in ('darwin', 'linux'):
        memory(binary, output / 'memory.csv', args.rss_samples)
    (output / 'provenance.json').write_text(
        json.dumps(
            {
                'timestamp': datetime.now(timezone.utc).isoformat(),
                'platform': platform.platform(),
                'compiler': subprocess.check_output(['rustc', '--version'], text=True).strip(),
                'command': command,
                'source_sha256': hashlib.sha256(source.read_bytes()).hexdigest(),
                'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
                'rss_samples': args.rss_samples,
            },
            indent=2,
        )
        + '\n'
    )


if __name__ == '__main__':
    if len(sys.argv) > 1 and sys.argv[1] == '--collect':
        collect(sys.argv[2:])
    else:
        main()
