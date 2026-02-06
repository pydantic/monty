# SQL Playground: Customer Sentiment Analysis

This example demonstrates using Monty for a task that **cannot be solved with a single SQL query**: analyzing customer purchase data (CSV) and correlating it with their social media sentiment (JSON tweets).

Data is from <https://github.com/mafudge/datasets>.

## Why This Example is Interesting

1. **Cross-format data joining**: CSV customer data must join with JSON tweets via Twitter handle - requires programmatic data wrangling
2. **Loop-based external calls**: Sentiment analysis for each tweet happens in a loop - with JSON tool calling this would flood the context window with 50+ results
3. **In-sandbox computation**: Averages, correlation, and aggregation happen in Python - no need for the LLM to do mental math
4. **Variable iteration**: Different customers have different numbers of tweets - code handles this naturally
5. **File sandboxing**: Uses `OSAccess` to mount data files, demonstrating secure file access patterns
6. **Type checking**: Validates LLM-generated code against type stubs before execution

## To run

```bash
uv run python examples/sql_playground/main.py
```
