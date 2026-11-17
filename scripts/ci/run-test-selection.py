#!/usr/bin/env python3
"""Validate libtest filters against the built binary before executing any tests."""
import argparse
import json
from pathlib import Path
import subprocess
import sys


def inventory(output, filters):
    names = sorted(line.removesuffix(': test') for line in output.splitlines()
                   if line.endswith(': test'))
    if not names or len(names) != len(set(names)):
        raise ValueError('empty or duplicate test inventory')
    if not filters or len(filters) != len(set(filters)):
        raise ValueError('empty or duplicate test filters')
    matches = {f: [n for n in names if f in n] for f in filters}
    selected = {n for group in matches.values() for n in group}
    return {'total': len(names), 'selected_count': len(selected),
            'filters': matches, 'empty_filters': [f for f, ns in matches.items() if not ns],
            'unselected': [n for n in names if n not in selected]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument('--manifest', type=Path)
    source.add_argument('--filter', action='append')
    parser.add_argument('--report', type=Path, required=True)
    parser.add_argument('--list-only', action='store_true')
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command
    if command[:1] == ['--']:
        command = command[1:]
    if not command:
        parser.error('a cargo test command or libtest binary is required after --')
    filters = args.filter or [line.strip() for line in args.manifest.read_text().splitlines()
                              if line.strip() and not line.lstrip().startswith('#')]
    # Cargo needs a separator for harness flags; a compiled libtest binary does not.
    separator = ['--'] if Path(command[0]).name == 'cargo' else []
    result = subprocess.run(command + separator + ['--list'], check=True,
                            stdout=subprocess.PIPE, text=True)
    report = inventory(result.stdout, filters)
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2) + '\n')
    print(f"Selected {report['selected_count']}/{report['total']} tests; "
          f"inventory: {args.report}", flush=True)
    if report['empty_filters']:
        raise ValueError(f"filters match zero tests: {report['empty_filters']}")
    if not args.list_only:
        for test_filter in filters:
            subprocess.run(command + [test_filter], check=True)


if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f'test selection failed: {error}', file=sys.stderr)
        sys.exit(1)
