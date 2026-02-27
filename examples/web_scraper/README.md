# Web Scraper Example

This example uses Python dataclass APIs for playwright and beautifulsoup to allow the LLM
to extract price data from the websites of model labs.

We use Pydantic AI to generate code, but instead of using the `CodeExecutionToolset` type from Pydantic AI,
we get the LLM to generate code directly allowing us to use new features of Monty not yet available in Pydantic AI.

Look at `example_code.py` for an example of the kind of code sonnet 4.5 will generate in this case.

Run the example with

```bash
uv run python -m examples.web_scraper.main
```
