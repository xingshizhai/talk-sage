import pytest
from unittest.mock import AsyncMock, MagicMock, patch
from llm.openai_compat import OpenAICompatProvider


@pytest.mark.asyncio
async def test_complete_returns_string():
    provider = OpenAICompatProvider(
        api_key="test-key",
        base_url="https://api.deepseek.com/v1",
        model="deepseek-chat",
    )
    mock_response = MagicMock()
    mock_response.choices = [MagicMock()]
    mock_response.choices[0].message.content = "NPI 是新产品导入流程"

    with patch.object(provider._client.chat.completions, "create", new=AsyncMock(return_value=mock_response)):
        result = await provider.complete(prompt="解释 NPI", system="你是一个助手")

    assert result == "NPI 是新产品导入流程"


@pytest.mark.asyncio
async def test_complete_with_empty_response():
    provider = OpenAICompatProvider(
        api_key="test-key",
        base_url="https://api.groq.com/openai/v1",
        model="llama3-70b-8192",
    )
    mock_response = MagicMock()
    mock_response.choices = [MagicMock()]
    mock_response.choices[0].message.content = ""

    with patch.object(provider._client.chat.completions, "create", new=AsyncMock(return_value=mock_response)):
        result = await provider.complete(prompt="test", system="test")

    assert result == ""
