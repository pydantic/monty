"""Fifty host functions generated from one spec, the Code Mode shape.

A fake SaaS API across CRM, tickets, calendar, billing and files: every tool is a
closure built from `_SPEC`, and the `.pyi` stubs the model sees are rendered from the
same spec, so the catalogue is as wide as a real MCP server's. The task needs six of
them, in an order that only the data reveals; the rest answer with canned values.
"""

from __future__ import annotations

import re
from collections.abc import Awaitable, Callable
from dataclasses import dataclass
from typing import Any

from evals.harness.evaluators import ApproxExpected
from evals.harness.task import Task

_CONTACTS: dict[str, dict[str, Any]] = {
    'c-1': {'contact_id': 'c-1', 'email': 'grace@acme.example', 'name': 'Grace Hopper', 'company_id': 'co-9'},
    'c-2': {'contact_id': 'c-2', 'email': 'alan@bletchley.example', 'name': 'Alan Turing', 'company_id': 'co-3'},
}
_COMPANIES: dict[str, dict[str, Any]] = {
    'co-9': {'company_id': 'co-9', 'name': 'Acme Ltd', 'plan': 'enterprise', 'owner': 'Ada'},
    'co-3': {'company_id': 'co-3', 'name': 'Bletchley Park', 'plan': 'team', 'owner': 'Edsger'},
}
_TICKETS: list[dict[str, Any]] = [
    {'ticket_id': 't-100', 'contact_id': 'c-1', 'status': 'open', 'subject': 'Invoice INV-2026-118 looks wrong'},
    {'ticket_id': 't-101', 'contact_id': 'c-1', 'status': 'closed', 'subject': 'Login loop'},
    {'ticket_id': 't-102', 'contact_id': 'c-2', 'status': 'open', 'subject': 'Export truncated'},
]
_INVOICES: dict[str, dict[str, Any]] = {
    'INV-2026-118': {'invoice_id': 'INV-2026-118', 'company_id': 'co-9', 'amount': 4820.5, 'status': 'overdue'},
    'INV-2026-119': {'invoice_id': 'INV-2026-119', 'company_id': 'co-3', 'amount': 990.0, 'status': 'paid'},
}

_state: dict[str, int] = {'notes': 0, 'events': 0}


def _reset() -> None:
    """Restart the note and event counters so ids repeat across attempts."""
    _state['notes'] = 0
    _state['events'] = 0


@dataclass(frozen=True)
class _ToolSpec:
    """One generated host function: its signature for the stubs and its behaviour."""

    name: str
    params: tuple[tuple[str, str], ...]
    returns: str
    doc: str
    impl: Callable[..., Any]

    def stub(self) -> str:
        params = ', '.join(f'{p}: {t}' for p, t in self.params)
        return f'async def {self.name}({params}) -> {self.returns}:\n    """{self.doc}"""\n    ...\n'


def _const(value: Any) -> Callable[..., Any]:
    """A tool that returns `value` whatever it is called with; the filler for unused tools."""

    def impl(*args: Any, **kwargs: Any) -> Any:
        return value

    return impl


def _lookup(table: dict[str, dict[str, Any]]) -> Callable[[str], dict[str, Any]]:
    """A tool that returns a copy of `table[key]`, or `{}` for an unknown key."""

    def impl(key: str) -> dict[str, Any]:
        return dict(table.get(key, {}))

    return impl


def _crm_find_contact(email: str) -> dict[str, Any]:
    matches = [dict(c) for c in _CONTACTS.values() if c['email'] == email]
    return matches[0] if matches else {}


def _tickets_list(contact_id: str, status: str = 'open') -> list[dict[str, Any]]:
    return [dict(t) for t in _TICKETS if t['contact_id'] == contact_id and t['status'] == status]


def _tickets_add_note(ticket_id: str, text: str) -> dict[str, Any]:
    _state['notes'] += 1
    return {'note_id': f'n-{_state["notes"]}', 'ticket_id': ticket_id, 'text': text}


def _calendar_create_event(title: str, start: str, minutes: int, attendees: list[str]) -> dict[str, Any]:
    _state['events'] += 1
    return {'event_id': f'ev-{_state["events"]}', 'title': title, 'start': start, 'attendees': list(attendees)}


def _spec(name: str, params: str, returns: str, doc: str, impl: Callable[..., Any] | None = None) -> _ToolSpec:
    """Build a spec from a `name: type, ...` parameter string; unused tools get a canned reply."""
    # Split between parameters only, not inside `dict[str, Any]`.
    pairs = [p.split(': ', 1) for p in re.split(r',\s*(?=\w+: )', params) if p.strip()]
    return _ToolSpec(name, tuple((p[0], p[1]) for p in pairs), returns, doc, impl or _const(_CANNED[returns]))


_CANNED: dict[str, Any] = {
    'dict[str, Any]': {'ok': True},
    'list[dict[str, Any]]': [],
    'list[str]': [],
    'str': '',
    'float': 0.2,
}

_SPEC: tuple[_ToolSpec, ...] = (
    # CRM
    _spec(
        'crm_find_contact', 'email: str', 'dict[str, Any]', 'Find a contact by email; `{}` if none.', _crm_find_contact
    ),
    _spec('crm_get_contact', 'contact_id: str', 'dict[str, Any]', 'A contact by id.', _lookup(_CONTACTS)),
    _spec('crm_list_contacts', 'company_id: str', 'list[dict[str, Any]]', 'Contacts at a company.'),
    _spec(
        'crm_get_company',
        'company_id: str',
        'dict[str, Any]',
        'A company: `company_id`, `name`, `plan`, `owner`.',
        _lookup(_COMPANIES),
    ),
    _spec('crm_search_companies', 'query: str', 'list[dict[str, Any]]', 'Companies whose name contains `query`.'),
    _spec('crm_update_contact', 'contact_id: str, fields: dict[str, Any]', 'dict[str, Any]', 'Update contact fields.'),
    _spec('crm_list_deals', 'company_id: str', 'list[dict[str, Any]]', 'Open deals for a company.'),
    _spec('crm_get_deal', 'deal_id: str', 'dict[str, Any]', 'A deal by id.'),
    _spec('crm_create_deal', 'company_id: str, value: float', 'dict[str, Any]', 'Create a deal.'),
    _spec(
        'crm_log_activity', 'contact_id: str, kind: str, text: str', 'dict[str, Any]', 'Log an activity on a contact.'
    ),
    # Tickets
    _spec(
        'tickets_list',
        'contact_id: str, status: str',
        'list[dict[str, Any]]',
        'Tickets for a contact with the given status (`open` or `closed`).',
        _tickets_list,
    ),
    _spec('tickets_get', 'ticket_id: str', 'dict[str, Any]', 'A ticket by id.'),
    _spec('tickets_create', 'contact_id: str, subject: str', 'dict[str, Any]', 'Open a ticket.'),
    _spec(
        'tickets_add_note',
        'ticket_id: str, text: str',
        'dict[str, Any]',
        'Add an internal note to a ticket; returns `note_id`.',
        _tickets_add_note,
    ),
    _spec('tickets_assign', 'ticket_id: str, agent: str', 'dict[str, Any]', 'Assign a ticket.'),
    _spec('tickets_close', 'ticket_id: str', 'dict[str, Any]', 'Close a ticket.'),
    _spec('tickets_search', 'query: str', 'list[dict[str, Any]]', 'Tickets whose subject contains `query`.'),
    _spec('tickets_list_notes', 'ticket_id: str', 'list[dict[str, Any]]', 'Notes on a ticket.'),
    _spec('tickets_set_priority', 'ticket_id: str, priority: str', 'dict[str, Any]', 'Set ticket priority.'),
    _spec('tickets_stats', 'period: str', 'dict[str, Any]', 'Ticket counts for a period.'),
    # Calendar
    _spec('calendar_list_events', 'start: str, end: str', 'list[dict[str, Any]]', 'Events between two ISO datetimes.'),
    _spec('calendar_get_event', 'event_id: str', 'dict[str, Any]', 'An event by id.'),
    _spec(
        'calendar_create_event',
        'title: str, start: str, minutes: int, attendees: list[str]',
        'dict[str, Any]',
        'Create an event; returns `event_id`.',
        _calendar_create_event,
    ),
    _spec('calendar_update_event', 'event_id: str, fields: dict[str, Any]', 'dict[str, Any]', 'Update an event.'),
    _spec('calendar_delete_event', 'event_id: str', 'dict[str, Any]', 'Delete an event.'),
    _spec('calendar_free_slots', 'attendees: list[str], date: str', 'list[str]', 'Free 30-minute slots on a date.'),
    _spec('calendar_list_calendars', '', 'list[str]', 'Calendar names.'),
    _spec('calendar_set_reminder', 'event_id: str, minutes_before: int', 'dict[str, Any]', 'Add a reminder.'),
    _spec('calendar_invite', 'event_id: str, email: str', 'dict[str, Any]', 'Invite someone.'),
    _spec('calendar_rsvp', 'event_id: str, status: str', 'dict[str, Any]', 'Respond to an invite.'),
    # Billing
    _spec(
        'billing_get_invoice',
        'invoice_id: str',
        'dict[str, Any]',
        'An invoice: `invoice_id`, `company_id`, `amount`, `status`.',
        _lookup(_INVOICES),
    ),
    _spec('billing_list_invoices', 'company_id: str', 'list[dict[str, Any]]', 'Invoices for a company.'),
    _spec('billing_create_invoice', 'company_id: str, amount: float', 'dict[str, Any]', 'Create an invoice.'),
    _spec('billing_record_payment', 'invoice_id: str, amount: float', 'dict[str, Any]', 'Record a payment.'),
    _spec('billing_list_payments', 'invoice_id: str', 'list[dict[str, Any]]', 'Payments on an invoice.'),
    _spec('billing_get_plan', 'company_id: str', 'dict[str, Any]', 'The billing plan for a company.'),
    _spec('billing_change_plan', 'company_id: str, plan: str', 'dict[str, Any]', 'Change a plan.'),
    _spec('billing_credit', 'company_id: str, amount: float, reason: str', 'dict[str, Any]', 'Issue a credit.'),
    _spec('billing_tax_rate', 'country: str', 'float', 'Tax rate for a country.'),
    _spec('billing_overdue', '', 'list[dict[str, Any]]', 'All overdue invoices.'),
    # Files
    _spec('files_list', 'folder: str', 'list[str]', 'File names in a folder.'),
    _spec('files_read', 'path: str', 'str', 'Read a text file.'),
    _spec('files_write', 'path: str, text: str', 'dict[str, Any]', 'Write a text file.'),
    _spec('files_delete', 'path: str', 'dict[str, Any]', 'Delete a file.'),
    _spec('files_share', 'path: str, email: str', 'dict[str, Any]', 'Share a file.'),
    _spec('files_search', 'query: str', 'list[str]', 'Files whose name contains `query`.'),
    _spec('files_move', 'path: str, folder: str', 'dict[str, Any]', 'Move a file.'),
    _spec('files_versions', 'path: str', 'list[dict[str, Any]]', 'Version history of a file.'),
    _spec('files_metadata', 'path: str', 'dict[str, Any]', 'Size and timestamps of a file.'),
    _spec('files_quota', '', 'dict[str, Any]', 'Storage used and available.'),
)
assert len(_SPEC) == 50, len(_SPEC)


def _as_tool(spec: _ToolSpec) -> Callable[..., Awaitable[Any]]:
    """An async host function whose behaviour is the spec's `impl`."""

    async def tool(*args: Any, **kwargs: Any) -> Any:
        return spec.impl(*args, **kwargs)

    tool.__name__ = spec.name
    return tool


TOOLS = {spec.name: _as_tool(spec) for spec in _SPEC}

STUBS = 'from typing import Any\n\n' + '\n'.join(spec.stub() for spec in _SPEC)

REFERENCE = """
contact = await crm_find_contact('grace@acme.example')
company = await crm_get_company(contact['company_id'])
open_tickets = await tickets_list(contact['contact_id'], 'open')
invoice = await billing_get_invoice('INV-2026-118')

ticket_id = open_tickets[0]['ticket_id']
note = await tickets_add_note(
    ticket_id,
    f"Invoice {invoice['invoice_id']} is {invoice['status']} for {invoice['amount']:.2f}; call booked.",
)
event = await calendar_create_event(
    f"Follow-up with {contact['name']}",
    '2026-09-15T10:00:00',
    30,
    [contact['email']],
)

{
    'contact_name': contact['name'],
    'plan': company['plan'],
    'open_tickets': [t['ticket_id'] for t in open_tickets],
    'invoice_amount': invoice['amount'],
    'note_id': note['note_id'],
    'event_id': event['event_id'],
}
"""

TASK = Task(
    name='fifty_tools',
    category='agents',
    prompt=(
        'grace@acme.example has emailed about invoice INV-2026-118. Using the tools available: '
        'find her contact record and her company, list her open tickets, fetch the invoice, add '
        'a note to her first open ticket that states the invoice id, its status and its amount '
        'to two decimal places and says a call is booked, and create a 30-minute calendar event '
        'titled "Follow-up with <her name>" at 2026-09-15T10:00:00 with her as the attendee. '
        'Return a dict with "contact_name", "plan", "open_tickets" (ticket ids), '
        '"invoice_amount", "note_id" and "event_id".'
    ),
    stubs=STUBS,
    tools=TOOLS,
    expected={
        'contact_name': 'Grace Hopper',
        'plan': 'enterprise',
        'open_tickets': ['t-100'],
        'invoice_amount': 4820.5,
        'note_id': 'n-1',
        'event_id': 'ev-1',
    },
    evaluators=(ApproxExpected(),),
    reference_solution=REFERENCE,
    traps=('picking six tools out of fifty', 'positional list argument', 'f-string format spec'),
    expected_external_calls=6,
    max_result_bytes=250,
    setup=_reset,
)
