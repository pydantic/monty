from __future__ import annotations

import json
from dataclasses import dataclass, field
from textwrap import indent
from typing import Any

from pydantic import BaseModel, ValidationError
from pydantic_ai import Agent, ModelRetry, format_as_xml


class ModelInfo(BaseModel, use_attribute_docstrings=True):
    unique_id: str
    """Unique identifier for the model."""
    name: str
    """Name of the model."""
    description: str | None = None
    """Description of the model."""
    input_mtok: float
    """Input tokens per million tokens."""
    output_mtok: float
    """Output tokens per million tokens."""
    attributes: dict[str, float | str] | None = None
    """Any other attributes of the model."""


agent = Agent(
    'gateway/anthropic:claude-sonnet-4-5',
    output_type=ModelInfo | str,
    instructions="Try to coerce the input into a ModelInfo object, if you're unable to do so, return a string describing the error.",
    name='extraction-sub-agent',
)


@dataclass
class RecordModels:
    models: dict[str, ModelInfo] = field(default_factory=dict)

    async def record_model_info(self, model_information: dict[str, Any]) -> str:
        """Record information about a model."""

        try:
            output = ModelInfo.model_validate(model_information)
        except ValidationError:
            pass
        else:
            self.models[output.unique_id] = output
            return f'Model information recorded successfully for {output.unique_id}'

        try:
            result = await agent.run(format_as_xml(model_information))
        except ModelRetry:
            return 'Error, unable to validate model information'
        else:
            output = result.output
            if isinstance(output, str):
                return output
            else:
                self.models[output.unique_id] = output
                return f'Model information recorded successfully for {output.unique_id}'

    @classmethod
    def record_model_info_stub(cls) -> str:
        return f'''\
from typing import Any

def record_model_info(model_information: dict[str, Any]) -> str:
    """Record information about a model.

    NOTE: this method takes a python dict argument, not JSON.

    The dict should have this schema:

    ```json
    {indent(json.dumps(ModelInfo.model_json_schema(), indent=2), prefix='    ')}
    ```
    """
    ...
'''
