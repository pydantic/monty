# call-external
# A class *body* that suspends on an external call. Because the class body now
# runs as a real, suspendable frame (Step 2), a class-variable value may call an
# external function: the VM yields to the host mid-definition and resumes,
# building the class with the resolved value.


class Config:
    # `add_ints` is an external function resolved by the host. Evaluated in the
    # class-body scope at class-definition time.
    limit = add_ints(40, 2)
    doubled = limit * 2

    def describe(self):
        return self.limit


assert Config.limit == 42
assert Config.doubled == 84
assert Config().describe() == 42
