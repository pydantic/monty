"""convert_value: transform values as they cross into the sandbox.

The hook sees every eager attribute and method return value. Use it to wrap
derived objects with their own policies (nothing is auto-wrapped — exposure
is always an explicit host decision) or to redact values. Each wrapper the
hook returns stays in the session's instance store until the session ends,
so a long-lived session calling `pay()` many times accumulates one entry per
call (see limitations/pool-architecture.md).
"""

from dataclasses import dataclass
from typing import Any

from pydantic_monty import ClassInstance, Monty


@dataclass
class Wallet:
    balance: int

    def pay(self, amount: int) -> 'Wallet':
        return Wallet(balance=self.balance - amount)


class WalletWrapper(ClassInstance):
    def convert_value(self, /, name: str, value: Any) -> Any:
        if isinstance(value, Wallet):
            return WalletWrapper(value, eager_attrs='all', allowed_methods={'pay'})
        return value


with Monty() as pool:
    with pool.checkout() as session:
        result = session.feed_run(
            'w.pay(30).pay(20).balance',
            inputs={'w': WalletWrapper(Wallet(100), eager_attrs='all', allowed_methods={'pay'})},
        )

assert result == 50
print(f'balance after two payments: {result}')
