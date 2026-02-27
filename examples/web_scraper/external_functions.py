import re
from dataclasses import dataclass, field
from typing import Any, Literal, cast

from bs4 import BeautifulSoup, Tag as BsTag

from .browser import PwPage, pw_pages


async def open_page(
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
    raise NotImplementedError('this is here just to generate stubs, see _generate_stubs in main.py')


@dataclass
class Page:
    """A snapshot of a Playwright page."""

    url: str
    title: str
    html: str
    id: int
    _pw_page: PwPage = field(init=False)

    def __post_init__(self):
        self._pw_page = pw_pages[self.id]

    async def go_to(
        self,
        url: str,
        wait_until: Literal['commit', 'domcontentloaded', 'load', 'networkidle'] = 'networkidle',
    ) -> None:
        """Navigate the page to a new URL.

        Args:
            url: The URL to navigate to.
            wait_until: When to consider navigation complete:
                `'commit'` — after the response is received,
                `'domcontentloaded'` — after the `DOMContentLoaded` event,
                `'load'` — after the `load` event,
                `'networkidle'` — after there are no network connections for 500ms.
        """
        await self._pw_page.goto(url, wait_until=wait_until)

    async def click(self, selector: str, force: bool = False) -> None:
        """Click an element matching the CSS selector and return the updated page.

        Args:
            selector: A CSS selector, e.g. `'button.submit'`, `'a[href="/next"]'`.
            force: If `True`, bypass actionability checks (visibility, pointer-events interception).
                Useful when an overlay or sticky nav covers the target element.
        """
        await self._pw_page.click(selector, force=force)
        await self._pw_page.wait_for_load_state('networkidle')

    async def fill(self, selector: str, value: str) -> None:
        """Fill a form field matching the CSS selector with the given value.

        Args:
            selector: A CSS selector for an input/textarea, e.g. `'input[name="email"]'`.
            value: The text to type into the field.
        """
        await self._pw_page.fill(selector, value)
        await self._pw_page.wait_for_load_state('networkidle')

    async def select_option(self, selector: str, value: str) -> None:
        """Select an option in a `<select>` element.

        Args:
            selector: A CSS selector for the select element.
            value: The value attribute of the option to select.
        """
        await self._pw_page.select_option(selector, value)
        await self._pw_page.wait_for_load_state('networkidle')

    async def check(self, selector: str) -> None:
        """Check a checkbox or radio button.

        Args:
            selector: A CSS selector for the checkbox/radio input.
        """
        await self._pw_page.check(selector)
        await self._pw_page.wait_for_load_state('networkidle')

    async def press(self, selector: str, key: str) -> None:
        """Press a keyboard key on a focused element.

        Args:
            selector: A CSS selector for the element to focus.
            key: Key to press, e.g. `'Enter'`, `'Tab'`, `'ArrowDown'`.
        """
        await self._pw_page.press(selector, key)
        await self._pw_page.wait_for_load_state('networkidle')

    async def wait_for_selector(self, selector: str, timeout: float = 30_000) -> None:
        """Wait for an element matching the CSS selector to appear, then return the updated page.

        Args:
            selector: A CSS selector to wait for.
            timeout: Maximum time to wait in milliseconds (default 30 000).
        """
        await self._pw_page.wait_for_selector(selector, timeout=timeout)
        await self._pw_page.wait_for_load_state('networkidle')

    async def screenshot(self, full_page: bool = False) -> bytes:
        """Take a screenshot of the current page.

        Args:
            full_page: If `True`, capture the full scrollable page rather than just the viewport.

        Returns:
            PNG image bytes.
        """
        return await self._pw_page.screenshot(full_page=full_page, type='png')

    async def evaluate(self, expression: str) -> str:
        """Evaluate a JavaScript expression on the page and return the result as a string.

        Args:
            expression: JavaScript to evaluate, e.g. `'document.title'`.
        """
        result = await self._pw_page.evaluate(expression)
        return str(result)

    async def get_text(self, selector: str) -> str:
        """Get the text content of the first element matching the CSS selector.

        Args:
            selector: A CSS selector, e.g. `'h1'`, `'.price'`.

        Returns:
            The text content of the matched element, or an empty string if not found.
        """
        text = await self._pw_page.text_content(selector)
        return text or ''

    async def get_attribute(self, selector: str, name: str) -> str | None:
        """Get an attribute value from the first element matching the CSS selector.

        Args:
            selector: A CSS selector.
            name: Attribute name, e.g. `'href'`, `'src'`.

        Returns:
            The attribute value, or `None` if the element or attribute is not found.
        """
        return await self._pw_page.get_attribute(selector, name)


def beautiful_soup(html: str) -> Tag:
    """Parse html with BeautifulSoup and return a `Tag`.

    Use this tool to get back a `Tag` object that can be used to extract information from HTML.
    """
    soup = BeautifulSoup(html, 'html.parser')
    return _from_beautifulsoup(soup)


@dataclass
class Tag:
    """A mirror of a BeautifulSoup `Tag`."""

    name: str
    attrs: dict[str, str | list[str]] = field(default_factory=dict)
    string: str | None = None
    text: str = ''
    html: str = ''

    def find(
        self, name: str | None = None, attrs: dict[str, str] | None = None, string: str | None = None
    ) -> 'Tag | None':
        """Find the first descendant tag matching the criteria.

        Args:
            name: Tag name to match, e.g. `'a'`, `'div'`.
            attrs: Attribute key-value pairs to filter on.
            string: Match tags whose `.string` equals this value.

        Returns:
            The first matching `Tag`, or `None` if no match is found.
        """
        # bs4's types are horrible, this is the easiest work around
        result = _parse(self.html).find(name, cast(Any, attrs), string=cast(Any, string))
        if result is None:
            return None
        else:
            return _from_beautifulsoup(result)

    def find_all(
        self,
        name: str | re.Pattern[str] | None = None,
        attrs: dict[str, str] | None = None,
        string: str | None = None,
        limit: int | None = None,
    ) -> 'list[Tag]':
        """Find all descendant tags matching the criteria.

        Args:
            name: Tag name or compiled regex to match.
            attrs: Attribute key-value pairs to filter on.
            string: Match tags whose `.string` equals this value.
            limit: Stop after finding this many results.

        Returns:
            A list of matching `Tag` objects.
        """
        # bs4's types are horrible, this is the easiest work around
        results = _parse(self.html).find_all(name, cast(Any, attrs), string=cast(Any, string), limit=limit)
        return [_from_beautifulsoup(r) for r in results]

    def select(self, selector: str) -> 'list[Tag]':
        """Find all descendants matching a CSS selector.

        Args:
            selector: A CSS selector string, e.g. `'div.class > a'`.

        Returns:
            A list of matching `Tag` objects.
        """
        return [_from_beautifulsoup(r) for r in _parse(self.html).select(selector)]

    def select_one(self, selector: str) -> 'Tag | None':
        """Find the first descendant matching a CSS selector.

        Args:
            selector: A CSS selector string, e.g. `'div.class > a'`.

        Returns:
            The first matching `Tag`, or `None` if no match is found.
        """
        result = _parse(self.html).select_one(selector)
        if result is None:
            return None
        return _from_beautifulsoup(result)

    def get(self, key: str, default: str | None = None) -> str | list[str] | None:
        """Get an attribute value by key.

        Args:
            key: The attribute name, e.g. `'href'`, `'class'`.
            default: Value to return if the attribute is missing.

        Returns:
            The attribute value (a `str`, or `list[str]` for multi-valued
            attributes like `class`), or `default` if not found.
        """
        return self.attrs.get(key, default)

    def get_text(self, separator: str = '', strip: bool = False) -> str:
        """Extract all text content within this tag.

        Args:
            separator: String inserted between text fragments.
            strip: Whether to strip whitespace from each fragment.

        Returns:
            The concatenated text content.
        """
        return _parse(self.html).get_text(separator=separator, strip=strip)

    def children(self) -> 'list[Tag | str]':
        """Get the direct children of this tag.

        Returns:
            A list where each element is either a `Tag` or a `str`
            (for navigable text nodes).
        """
        out: list[Tag | str] = []
        for child in _parse(self.html).children:
            if isinstance(child, BsTag):
                out.append(_from_beautifulsoup(child))
            else:
                text = str(child)
                if text:
                    out.append(text)
        return out


# ------------------------------------------------------------------
# Public helpers
# ------------------------------------------------------------------


def _from_beautifulsoup(element: BsTag) -> Tag:
    """Convert a BeautifulSoup `Tag` into a Monty-compatible `Tag` dataclass."""
    assert isinstance(element, BsTag), f'Expected a BeautifulSoup Tag, got {type(element)}'
    string_val = element.string
    return Tag(
        name=element.name,
        attrs=dict(element.attrs),
        string=str(string_val) if string_val is not None else None,
        text=element.get_text(),
        html=str(element),
    )


def _parse(html: str) -> BsTag:
    """Re-parse stored HTML into a BeautifulSoup tag.

    If the HTML represents a full document the `BeautifulSoup` object
    itself is returned (it behaves like a Tag).  Otherwise the first
    child tag is returned so that `find`/`select` operate on the
    correct element.
    """
    soup = BeautifulSoup(html, 'html.parser')
    # If the html was a single tag, unwrap so searches are scoped correctly.
    children = list(soup.children)
    if len(children) == 1 and isinstance(children[0], BsTag):
        return children[0]
    return soup
