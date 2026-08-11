class Account:
    def __init__(self, balance: int) -> None:
        self.balance = balance

    def withdraw(self, amount: int) -> None:
        if amount > self.balance:
            raise ValueError('insufficient funds')
        self.balance -= amount


a = Account(100)
a.withdraw(200)
"""
TRACEBACK:
Traceback (most recent call last):
  File "class__method_raises.py", line 12, in <module>
    a.withdraw(200)
    ~~~~~~~~~~~~~~~
  File "class__method_raises.py", line 7, in withdraw
    raise ValueError('insufficient funds')
ValueError: insufficient funds
"""
