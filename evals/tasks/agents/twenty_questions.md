# twenty_questions

Twenty questions with the questioner loop in the sandbox, from the PyCon Italy demo.
The code holds the conversation, asks `call_llm` for the next yes/no question, puts it to `ask_oracle`, appends both,
and stops when a guess of the form "Is it a ...?" is answered "yes" or after twenty questions.
Return `answer` (the guessed object) and `questions` (how many were asked).

`ask_oracle` answers from a keyword table about the secret (a kettle).
The dry-run stub for `call_llm` counts the `assistant:` lines so far and returns the next question of a six-step
script that ends with the right guess.

Scored with `EqualsExpected` against `{'answer': 'kettle', 'questions': 6}`, `result_size` (100 bytes), and a pin of
twelve host calls in twelve round trips.
