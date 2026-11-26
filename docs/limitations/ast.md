# `ast` module

Monty exposes `ast.literal_eval(node_or_string)` for string input.
The AST node classes, `ast.parse()`, and the rest of the module are unavailable.

- A non-string argument raises `ValueError` without calling its `__repr__`.
- A `ValueError` for an unsupported expression omits the AST node's representation.
- Syntax diagnostics use Monty's parser messages. Invalid indentation raises `SyntaxError`.
- Ruff rejects some Unicode character names that CPython accepts. For example, `\N{TANGUT IDEOGRAPH-17000}` raises
    `SyntaxError`. Use the numeric escape `\U00017000` for the same character.
- Lone surrogate escapes such as `\ud800` decode to U+FFFD because Monty strings store UTF-8.
- Monty's source nesting limit bounds parsing and conversion. A deeply nested input can raise `SyntaxError`.
