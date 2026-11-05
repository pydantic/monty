"""Install only the scenario test harness in a separate checkout of PR #1236."""

import subprocess
import sys
from pathlib import Path

candidate = Path(__file__).resolve().parents[3] / 'monty/src/types/str'
baseline = Path(sys.argv[1]).resolve()
revision = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=baseline, text=True).strip()
if revision != '5c9dddf39c92981b8ef023be059e8c3aeabf7e91':
    raise SystemExit('Use a separate checkout of PR #1236 at 5c9dddf39c92981b8ef023be059e8c3aeabf7e91')

destination = baseline / 'crates/monty/src/types/str'
harness = (candidate / 'str_view_bench.rs').read_text()
harness = harness.replace(
    'use super::{MIN_VIEW_LEN, MIN_VIEW_RATIO, StringStorage};',
    'const MIN_VIEW_LEN: usize = 128;\nconst MIN_VIEW_RATIO: usize = 4;',
)
# The baseline has no backing probes; keep its production string implementation intact.
start = harness.index('pub(super) fn allocated')
end = harness.index('#[derive(Clone, Copy)]\nstruct Policy', start)
harness = harness[:start] + harness[end:]
harness = harness.replace('sync::Arc, ', '')
(destination / 'str_view_bench.rs').write_text(harness)

scenarios = (candidate / 'str_realistic_bench.rs').read_text()
start = scenarios.index('    let policies = vec![')
end = scenarios.index('    run_matrix', start)
scenarios = (
    scenarios[:start]
    + """    let policies = vec![Policy { name: "base", minimum: usize::MAX, ratio: 1 }];
"""
    + scenarios[end:]
)
(destination / 'str_realistic_bench.rs').write_text(scenarios)

module = destination / 'mod.rs'
text = module.read_text()
if 'mod str_view_bench;' not in text:
    module.write_text(text + '\n#[cfg(test)]\nmod str_view_bench;\n')
