from __future__ import annotations

from contextlib import asynccontextmanager
from dataclasses import dataclass
from typing import TYPE_CHECKING, AsyncIterator, Literal

from playwright.async_api import Browser as PwBrowser, Page as PwPage, async_playwright

if TYPE_CHECKING:
    from .external_functions import Page

pw_pages: dict[int, PwPage] = {}


@asynccontextmanager
async def start_browser() -> AsyncIterator[Browser]:
    async with async_playwright() as p:
        b = await p.chromium.launch()
        yield Browser(b)
        pw_pages.clear()
        await b.close()


@dataclass
class Browser:
    _pw_browser: PwBrowser

    async def open_page(
        self,
        url: str,
        wait_until: Literal['commit', 'domcontentloaded', 'load', 'networkidle'] = 'networkidle',
    ) -> Page:
        """Open a URL in a headless browser and return a `Page`.

        Use this to load a web page so you can inspect its HTML content.

        Args:
            url: The URL to navigate to.
            wait_until: When to consider navigation complete:
                `'commit'` — after the response is received,
                `'domcontentloaded'` — after the `DOMContentLoaded` event,
                `'load'` — after the `load` event,
                `'networkidle'` — after there are no network connections for 500ms.
        """
        from .external_functions import Page

        page = await self._pw_browser.new_page()
        await page.goto(url, wait_until=wait_until)
        page_id = id(page)
        pw_pages[page_id] = page
        return Page(
            url=page.url,
            title=await page.title(),
            html=await page.content(),
            id=page_id,
        )
