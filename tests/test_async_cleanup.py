from __future__ import annotations

import unittest

from unittest.mock import patch

from oy_cli import providers


class OpenAIPairTests(unittest.TestCase):
    def test_openai_pair_supplies_owned_http_clients(self):
        async_http = object()
        sync_http = object()

        with (
            patch("oy_cli.providers.async_http_client", return_value=async_http) as make_async_http,
            patch("oy_cli.providers.http_client", return_value=sync_http) as make_sync_http,
            patch("oy_cli.providers.AsyncOpenAI") as async_openai,
            patch("oy_cli.providers.OpenAI") as openai,
        ):
            providers._openai_pair(
                "test-key",
                base_url="https://example.com/v1",
                max_retries=7,
                timeout=12,
            )

        make_async_http.assert_called_once_with(
            api_key="test-key",
            base_url="https://example.com/v1",
            timeout=12,
        )
        make_sync_http.assert_called_once_with(
            api_key="test-key",
            base_url="https://example.com/v1",
            timeout=12,
        )
        async_openai.assert_called_once_with(
            api_key="test-key",
            base_url="https://example.com/v1",
            max_retries=7,
            timeout=12,
            http_client=async_http,
        )
        openai.assert_called_once_with(
            api_key="test-key",
            base_url="https://example.com/v1",
            max_retries=7,
            timeout=12,
            http_client=sync_http,
        )

    def test_copilot_openai_pair_supplies_owned_http_clients(self):
        async_http = object()
        sync_http = object()

        with (
            patch("oy_cli.providers.async_http_client", return_value=async_http) as make_async_http,
            patch("oy_cli.providers.http_client", return_value=sync_http) as make_sync_http,
            patch("oy_cli.providers.AsyncOpenAI") as async_openai,
            patch("oy_cli.providers.OpenAI") as openai,
        ):
            providers._copilot_openai_pair("test-token")

        expected = {
            "api_key": "test-token",
            "base_url": providers._COPILOT_BASE_URL,
            "default_headers": providers._copilot_default_headers(),
        }
        make_async_http.assert_called_once_with(**expected)
        make_sync_http.assert_called_once_with(**expected)
        async_openai.assert_called_once_with(
            **expected,
            max_retries=0,
            http_client=async_http,
        )
        openai.assert_called_once_with(
            **expected,
            max_retries=0,
            http_client=sync_http,
        )


