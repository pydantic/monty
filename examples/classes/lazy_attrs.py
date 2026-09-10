"""ClassInstance lazy attributes: fetched from the host on demand.

`lazy_attrs` names cross only when sandbox code reads them — each access
suspends the sandbox and asks the host. Anything outside the policy raises
the usual `AttributeError` inside the sandbox.
"""

from pydantic_monty import ClassInstance, Monty, MontyRuntimeError


class Config:
    def __init__(self) -> None:
        self.retries = 3
        self.api_key = 'hunter2'  # never exposed below


with Monty() as pool:
    with pool.checkout() as session:
        wrapper = ClassInstance(Config(), lazy_attrs={'retries'})
        assert session.feed_run('cfg.retries', inputs={'cfg': wrapper}) == 3

    with pool.checkout() as session:
        try:
            session.feed_run('cfg.api_key', inputs={'cfg': ClassInstance(Config(), lazy_attrs={'retries'})})
        except MontyRuntimeError as exc:
            print(f'denied as expected: {exc}')
        else:
            raise AssertionError('expected api_key to be denied')
