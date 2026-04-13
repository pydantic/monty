import pytest
from pytest_examples import CodeExample, EvalExample, find_examples

paths = (
    'crates/monty-python/README.md',
    'README.md',
)


@pytest.mark.parametrize('example', find_examples(*paths), ids=str)
def test_readme_examples(example: CodeExample, eval_example: EvalExample):
    eval_example.set_config(target_version='py310', ruff_ignore=['FA102'])
    eval_example.lint(example)
    opt_test = example.prefix_settings().get('test', '')
    if opt_test.startswith('skip'):
        pytest.skip(opt_test[4:].lstrip(' -') or 'running code skipped')
    if eval_example.update_examples:
        eval_example.run_print_update(example)
    else:
        eval_example.run_print_check(example)
