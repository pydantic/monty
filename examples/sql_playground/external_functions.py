from __future__ import annotations

import json
import tempfile
from dataclasses import dataclass
from pathlib import PurePosixPath
from typing import Any

from pydantic_monty import OSAccess

try:
    import duckdb
except ImportError as e:
    raise ImportError('duckdb is required for query_csv. Install with: pip install duckdb') from e


@dataclass
class ExternalFunctions:
    fs: OSAccess

    async def query_csv(
        self, filepath: PurePosixPath, sql: str, parameters: dict[str, Any] | None = None
    ) -> list[dict[str, Any]]:
        """Execute SQL query on a CSV file using DuckDB.

        Args:
            filepath: Path to the CSV file in the virtual filesystem.
            sql: SQL query to execute. The CSV data is available as a table named 'data'.
            parameters: Optional dictionary of parameters to bind to the SQL query.

        Returns:
            List of dictionaries, one per row, with column names as keys.
        """
        # Read CSV content from virtual filesystem
        content = self.fs.path_read_bytes(filepath)

        # Write to a temporary file for DuckDB to read
        # (DuckDB's read_csv_auto works best with file paths)
        with tempfile.NamedTemporaryFile(mode='wb', suffix='.csv') as tmp:
            tmp.write(content)
            tmp.flush()

            conn = duckdb.connect(':memory:')
            # Create table from CSV
            # NOTE! duckdb (horribly) reads locals as tables, hence `data` here that isn't used
            data = conn.read_csv(tmp.name)
            # Execute the user's query
            result_rel = conn.execute(sql, parameters)
            del data
        # Get column names and rows, then convert to list of dicts
        columns = [desc[0] for desc in result_rel.description]
        rows = result_rel.fetchall()
        return [dict(zip(columns, row)) for row in rows]

    async def read_json(self, filepath: PurePosixPath) -> list[Any] | dict[str, Any]:
        """Read and parse a JSON file from the virtual filesystem.

        Args:
            filepath: Path to the JSON file in the virtual filesystem.

        Returns:
            Parsed JSON data (list or dict).
        """
        content = self.fs.path_read_text(filepath)
        return json.loads(content)

    @staticmethod
    async def analyze_sentiment(text: str) -> float:
        """Analyze sentiment of text using simple keyword matching.

        This is a basic sentiment analyzer that scores text based on
        the presence of positive and negative keywords. For production use,
        you would want to use a proper NLP library or API.

        Args:
            text: The text to analyze.

        Returns:
            Sentiment score from -1.0 (very negative) to +1.0 (very positive).
            A score of 0.0 indicates neutral sentiment.

        Example:
            >>> await analyze_sentiment('This product is amazing!')
            0.3
        """
        positive_words = [
            'amazing',
            'great',
            'love',
            'thank',
            'helpful',
            'a+',
            'good',
            'best',
            'excellent',
            'awesome',
            'fantastic',
            'wonderful',
            'glad',
            'enjoy',
            'better',
        ]
        negative_words = [
            'bad',
            'angry',
            'hate',
            'terrible',
            'worst',
            'fraud',
            'awful',
            'horrible',
            'disappointed',
            'poor',
            'useless',
        ]

        score = 0.0
        text_lower = text.lower()

        for word in positive_words:
            if word in text_lower:
                score += 0.3

        for word in negative_words:
            if word in text_lower:
                score -= 0.3

        # Clamp score to [-1, 1]
        return max(-1.0, min(1.0, score))
