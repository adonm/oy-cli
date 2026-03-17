from __future__ import annotations

import unittest
from unittest.mock import patch

import shim


class OpenAIPairTests(unittest.TestCase):
    def test_openai_pair_supplies_owned_http_clients(self):
        async_http = object()
        sync_http = object()

        with (
            patch("shim.async_http_client", return_value=async_http) as make_async_http,
            patch("shim.http_client", return_value=sync_http) as make_sync_http,
            patch("shim.AsyncOpenAI") as async_openai,
            patch("shim.OpenAI") as openai,
        ):
            shim._openai_pair(
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
            patch("shim.async_http_client", return_value=async_http) as make_async_http,
            patch("shim.http_client", return_value=sync_http) as make_sync_http,
            patch("shim.AsyncOpenAI") as async_openai,
            patch("shim.OpenAI") as openai,
        ):
            shim._copilot_openai_pair("test-token")

        expected = {
            "api_key": "test-token",
            "base_url": shim._COPILOT_BASE_URL,
            "default_headers": shim._copilot_default_headers(),
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
