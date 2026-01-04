from typing import Any

import pytest
from inline_snapshot import snapshot

import monty


def test_start_no_external_functions_returns_complete():
    m = monty.Monty('1 + 2')
    result = m.start()
    assert isinstance(result, monty.MontyComplete)
    assert result.output == snapshot(3)


def test_start_with_external_function_returns_progress():
    m = monty.Monty('func()', external_functions=['func'])
    result = m.start()
    assert isinstance(result, monty.MontySnapshot)
    assert result.script_name == snapshot('main.py')
    assert result.function_name == snapshot('func')
    assert result.args == snapshot(())
    assert result.kwargs == snapshot({})


def test_start_custom_script_name():
    m = monty.Monty('func()', script_name='custom.py', external_functions=['func'])
    result = m.start()
    assert isinstance(result, monty.MontySnapshot)
    assert result.script_name == snapshot('custom.py')


def test_start_progress_resume_returns_complete():
    m = monty.Monty('func()', external_functions=['func'])
    progress = m.start()
    assert isinstance(progress, monty.MontySnapshot)
    assert progress.function_name == snapshot('func')
    assert progress.args == snapshot(())
    assert progress.kwargs == snapshot({})

    result = progress.resume(42)
    assert isinstance(result, monty.MontyComplete)
    assert result.output == snapshot(42)


def test_start_progress_with_args():
    m = monty.Monty('func(1, 2, 3)', external_functions=['func'])
    progress = m.start()
    assert isinstance(progress, monty.MontySnapshot)
    assert progress.function_name == snapshot('func')
    assert progress.args == snapshot((1, 2, 3))
    assert progress.kwargs == snapshot({})


def test_start_progress_with_kwargs():
    m = monty.Monty('func(a=1, b="two")', external_functions=['func'])
    progress = m.start()
    assert isinstance(progress, monty.MontySnapshot)
    assert progress.function_name == snapshot('func')
    assert progress.args == snapshot(())
    assert progress.kwargs == snapshot({'a': 1, 'b': 'two'})


def test_start_progress_with_mixed_args_kwargs():
    m = monty.Monty('func(1, 2, x="hello", y=True)', external_functions=['func'])
    progress = m.start()
    assert isinstance(progress, monty.MontySnapshot)
    assert progress.function_name == snapshot('func')
    assert progress.args == snapshot((1, 2))
    assert progress.kwargs == snapshot({'x': 'hello', 'y': True})


def test_start_multiple_external_calls():
    m = monty.Monty('a() + b()', external_functions=['a', 'b'])

    # First call
    progress = m.start()
    assert isinstance(progress, monty.MontySnapshot)
    assert progress.function_name == snapshot('a')

    # Resume with first return value
    progress = progress.resume(10)
    assert isinstance(progress, monty.MontySnapshot)
    assert progress.function_name == snapshot('b')

    # Resume with second return value
    result = progress.resume(5)
    assert isinstance(result, monty.MontyComplete)
    assert result.output == snapshot(15)


def test_start_chain_of_external_calls():
    m = monty.Monty('c() + c() + c()', external_functions=['c'])

    call_count = 0
    progress: monty.MontySnapshot | monty.MontyComplete = m.start()

    while isinstance(progress, monty.MontySnapshot):
        assert progress.function_name == snapshot('c')
        call_count += 1
        progress = progress.resume(call_count)

    assert isinstance(progress, monty.MontyComplete)
    assert progress.output == snapshot(6)  # 1 + 2 + 3
    assert call_count == snapshot(3)


def test_start_with_inputs():
    m = monty.Monty('process(x)', inputs=['x'], external_functions=['process'])
    progress = m.start(inputs={'x': 100})
    assert isinstance(progress, monty.MontySnapshot)
    assert progress.function_name == snapshot('process')
    assert progress.args == snapshot((100,))


def test_start_with_limits():
    m = monty.Monty('1 + 2')
    limits = monty.ResourceLimits(max_allocations=1000)
    result = m.start(limits=limits)
    assert isinstance(result, monty.MontyComplete)
    assert result.output == snapshot(3)


def test_start_with_print_callback():
    output: list[tuple[str, str]] = []

    def callback(stream: str, text: str) -> None:
        output.append((stream, text))

    m = monty.Monty('print("hello")')
    result = m.start(print_callback=callback)
    assert isinstance(result, monty.MontyComplete)
    assert output == snapshot([('stdout', 'hello'), ('stdout', '\n')])


def test_start_resume_cannot_be_called_twice():
    m = monty.Monty('func()', external_functions=['func'])
    progress = m.start()
    assert isinstance(progress, monty.MontySnapshot)

    # First resume succeeds
    progress.resume(1)

    # Second resume should fail
    with pytest.raises(RuntimeError) as exc_info:
        progress.resume(2)
    assert exc_info.value.args[0] == snapshot('Progress already resumed')


def test_start_complex_return_value():
    m = monty.Monty('func()', external_functions=['func'])
    progress = m.start()
    assert isinstance(progress, monty.MontySnapshot)

    result = progress.resume({'a': [1, 2, 3], 'b': {'nested': True}})
    assert isinstance(result, monty.MontyComplete)
    assert result.output == snapshot({'a': [1, 2, 3], 'b': {'nested': True}})


def test_start_resume_with_none():
    m = monty.Monty('func()', external_functions=['func'])
    progress = m.start()
    assert isinstance(progress, monty.MontySnapshot)

    result = progress.resume(None)
    assert isinstance(result, monty.MontyComplete)
    assert result.output is None


def test_progress_repr():
    m = monty.Monty('func(1, x=2)', external_functions=['func'])
    progress = m.start()
    assert isinstance(progress, monty.MontySnapshot)
    assert repr(progress) == snapshot(
        "MontySnapshot(script_name='main.py', function_name='func', args=(1,), kwargs={'x': 2})"
    )


def test_complete_repr():
    m = monty.Monty('42')
    result = m.start()
    assert isinstance(result, monty.MontyComplete)
    assert repr(result) == snapshot('MontyComplete(output=42)')


def test_start_can_reuse_monty_instance():
    m = monty.Monty('func(x)', inputs=['x'], external_functions=['func'])

    # First run
    progress1 = m.start(inputs={'x': 1})
    assert isinstance(progress1, monty.MontySnapshot)
    assert progress1.args == snapshot((1,))
    result1 = progress1.resume(10)
    assert isinstance(result1, monty.MontyComplete)
    assert result1.output == snapshot(10)

    # Second run with different input
    progress2 = m.start(inputs={'x': 2})
    assert isinstance(progress2, monty.MontySnapshot)
    assert progress2.args == snapshot((2,))
    result2 = progress2.resume(20)
    assert isinstance(result2, monty.MontyComplete)
    assert result2.output == snapshot(20)


@pytest.mark.parametrize(
    'code,expected',
    [
        ('1', 1),
        ('"hello"', 'hello'),
        ('[1, 2, 3]', [1, 2, 3]),
        ('{"a": 1}', {'a': 1}),
        ('None', None),
        ('True', True),
    ],
)
def test_start_returns_complete_for_various_types(code: str, expected: Any):
    m = monty.Monty(code)
    result = m.start()
    assert isinstance(result, monty.MontyComplete)
    assert result.output == expected
