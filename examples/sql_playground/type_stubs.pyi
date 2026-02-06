from pathlib import Path
from typing import Any

async def query_csv(filepath: Path, sql: str, parameters: dict[str, Any] | None = None) -> list[dict[str, Any]]:
    """Execute SQL query on a CSV file using DuckDB."""
    ...

async def read_json(filepath: Path) -> list[Any] | dict[str, Any]:
    """Read and parse a JSON file."""
    ...

async def analyze_sentiment(text: str) -> float:
    """Analyze sentiment of text. Returns score from -1.0 to +1.0."""
    ...
