# Regression coverage for the `global X` ordering diagnostics, including the
# CPython quirk reported in https://github.com/pydantic/monty/issues/423:
# `import` bindings DO NOT count as "assigned to before global declaration"
# even though every other binding form does.
#
# Each `def` below stresses one rule. The function bodies are never executed
# — defining them is enough to trigger prepare-time scope validation. The
# file ends with the one form that IS expected to raise a SyntaxError, which
# the harness verifies through the `# Raise=` comment.


# --- Import bindings are accepted by CPython ---------------------------
# `import X`, `import X as Y`, and `from M import X [as Y]` are treated as
# "soft" bindings that don't conflict with a subsequent `global` in the same
# scope (issue #423). The `global` declaration takes precedence, making the
# import effectively rebind the module-level name.


def import_plain_then_global():
    import os

    global os  # type: ignore[reportAssignmentBeforeGlobalDeclaration]


def import_as_then_global():
    import os as alias_plain

    global alias_plain  # type: ignore[reportAssignmentBeforeGlobalDeclaration]


def from_import_then_global():
    from os import path

    global path  # type: ignore[reportAssignmentBeforeGlobalDeclaration]


def from_import_as_then_global():
    from os import path as alias_path

    global alias_path  # type: ignore[reportAssignmentBeforeGlobalDeclaration]


# --- Reads inside a nested scope don't pollute this scope --------------
# Reads inside a `lambda` body or nested `def` happen in a sub-scope, so
# they don't trigger this scope's "used prior to" check.


def lambda_read_then_global():
    lambda: lambda_target
    global lambda_target


def nested_def_read_then_global():
    def nested_reader():
        return nested_target

    _ = nested_reader  # keep pyright happy about the nested def
    global nested_target

    def nested_target():
        pass


# --- The non-import binding form that DOES error -----------------------
# A plain assignment before `global` is the canonical SyntaxError.


def f():
    x = 1
    global x  # type: ignore[reportAssignmentBeforeGlobalDeclaration]


f()
# Raise=SyntaxError("name 'x' is assigned to before global declaration")
