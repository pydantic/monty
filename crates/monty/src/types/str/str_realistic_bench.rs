//! Application-shaped fixtures; template and XML kernels are not full package integrations.
use std::{env, fmt::Write, fs, path::PathBuf};

use serde_json::{Value as Json, json};

use super::{Case, Policy, Task, run_matrix};

const TEMPLATE: &str = r"
context = {'customer': 'Zoë & Partners', 'reference': 'INV-2026-0042', 'total': '1234.50 EUR'}
parts = []
position = 0
while True:
    start = data.find('{{', position)
    if start < 0:
        parts.append(data[position:])
        break
    parts.append(data[position:start])
    end = data.find('}}', start + 2)
    key = data[start + 2:end].strip()
    value = context[key].replace('&', '&amp;').replace('<', '&lt;').replace('>', '&gt;')
    parts.append(value)
    position = end + 2
result = ''.join(parts)
data = None
len(result)
";

const XML: &str = r"
parts = []
position = 0
while True:
    start = data.find('<w:t', position)
    if start < 0:
        break
    start = data.find('>', start) + 1
    end = data.find('</w:t>', start)
    text = data[start:end]
    if '&' in text:
        text = text.replace('&lt;', '<').replace('&gt;', '>').replace('&amp;', '&')
    parts.append(text)
    position = end + 6
";

const JSON_REQUEST: &str = r"
import json
request = json.loads(data)
data = None
total = 0
errors = []
for event in request['events']:
    total += event['amount']
    if not event['ok']:
        errors.append({'id': event['id'], 'preview': event['message'][:160]})
response = json.dumps(errors, ensure_ascii=False, separators=(',', ':'))
total + len(response)
";

const NDJSON_TAIL: &str = r"
import json
buffer = data
data = None
total = 0
errors = 0
while buffer:
    end = buffer.find(chr(10))
    if end < 0:
        line = buffer
        buffer = ''
    else:
        line = buffer[:end]
        buffer = buffer[end + 1:]
    event = json.loads(line)
    total += event['amount']
    if not event['ok']:
        errors += 1
total + errors
";

const NDJSON_LINES: &str = r"
import json
lines = data.splitlines()
data = None
total = 0
errors = 0
for line in lines:
    event = json.loads(line)
    total += event['amount']
    if not event['ok']:
        errors += 1
total + errors
";

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn ascii_json(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii() {
            escaped.push(c);
        } else {
            let mut units = [0; 2];
            for unit in c.encode_utf16(&mut units) {
                write!(escaped, "\\u{unit:04x}").unwrap();
            }
        }
    }
    escaped
}

fn template_fixture(styles: bool) -> (String, i64) {
    let mut source = String::from("<!doctype html><html><head><style>");
    if styles {
        for i in 0..700 {
            writeln!(
                source,
                ".field-{i} {{ font: 14px sans-serif; padding: 8px 12px; border-bottom: 1px solid #eee; }}"
            )
            .unwrap();
        }
    }
    source.push_str("</style></head><body>");
    for _ in 0..300 {
        source.push_str(
            "<section><h2>{{ customer }}</h2><p>Reference {{ reference }}</p><strong>{{ total }}</strong></section>\n",
        );
    }
    source.push_str("</body></html>");
    let rendered = source
        .replace("{{ customer }}", &escape("Zoë & Partners"))
        .replace("{{ reference }}", "INV-2026-0042")
        .replace("{{ total }}", "1234.50 EUR");
    (source, i64::try_from(rendered.chars().count()).unwrap())
}

fn xml_fixture() -> (String, Vec<String>) {
    let mut xml =
        String::from("<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body>");
    let mut texts = Vec::new();
    for paragraph in 0..160 {
        xml.push_str("<w:p><w:pPr><w:pStyle w:val=\"BodyText\"/></w:pPr>");
        for run in 0..4 {
            let text = format!(
                "Clause {paragraph}.{run}: München & Zürich approve the revised budget <2026>; delivery remains scheduled for next quarter. "
            );
            write!(xml, "<w:r><w:t xml:space=\"preserve\">{}</w:t></w:r>", escape(&text)).unwrap();
            texts.push(text);
        }
        xml.push_str("</w:p>");
    }
    xml.push_str("</w:body></w:document>");
    (xml, texts)
}

fn event_fixture() -> (Vec<Json>, i64, Vec<Json>) {
    let mut events = Vec::new();
    let mut errors = Vec::new();
    let mut total = 0;
    for i in 0..2000 {
        let amount = 100 + i % 1000;
        let ok = i % 17 != 0;
        let id = format!("019d0000-0000-7000-8000-{i:012x}");
        let message = format!("Order {i}: München → Zürich. Customer requested delivery next week; verify the address and account reference before dispatch. ").repeat(3);
        let event = json!({"id":id,"account":format!("acct-{:06}", i % 100),"amount":amount,"ok":ok,"message":message});
        if !ok {
            errors.push(json!({"id":id,"preview":message.chars().take(160).collect::<String>()}));
        }
        total += amount;
        events.push(event);
    }
    (events, total, errors)
}

fn cases() -> Vec<Case> {
    let mut cases = Vec::new();
    for (name, styles) in [("template_styled", true), ("template_fragments", false)] {
        let (source, expected) = template_fixture(styles);
        cases.push(Case::input(name, TEMPLATE, source, expected, false));
    }
    let (xml, texts) = xml_fixture();
    let expected = i64::try_from(texts.iter().map(|s| s.chars().count()).sum::<usize>()).unwrap();
    let consume = format!("{XML}\nresult = ''.join(parts)\nparts = None\ntext = None\ndata = None\nlen(result)");
    cases.push(Case::input("docx_text", &consume, xml.clone(), expected, false));
    let retain = format!(
        "{XML}\nselected = parts[::64]\nparts = None\ntext = None\ndata = None\nsum(len(text) for text in selected)"
    );
    let selected = i64::try_from(texts.iter().step_by(64).map(|s| s.chars().count()).sum::<usize>()).unwrap();
    cases.push(Case::input("docx_selected", &retain, xml, selected, true));

    let (events, total, errors) = event_fixture();
    let body = serde_json::to_string(&json!({"tenant":"warehouse-eu","events":events})).unwrap();
    let response_len = i64::try_from(serde_json::to_string(&errors).unwrap().chars().count()).unwrap();
    cases.push(Case::input(
        "json_request",
        JSON_REQUEST,
        body,
        total + response_len,
        false,
    ));
    let mut uri_events = events.clone();
    let mut uri_errors = Vec::new();
    let mut uri_total = 0;
    for event in &mut uri_events {
        let id = event["id"].as_str().unwrap().to_owned();
        let uri = format!("s3://warehouse-eu/2026/11/05/orders/{id}/invoice.pdf");
        let path = uri[5..].to_owned();
        uri_total += event["amount"].as_i64().unwrap() + i64::try_from(path.len()).unwrap();
        if !event["ok"].as_bool().unwrap() {
            uri_errors.push(json!({"id":id,"preview":event["message"].as_str().unwrap().chars().take(160).collect::<String>(),"path":path}));
        }
        event["object_uri"] = Json::String(uri);
    }
    let uri_body = serde_json::to_string(&json!({"tenant":"warehouse-eu","events":uri_events})).unwrap();
    let uri_source = JSON_REQUEST.replace("    total += event['amount']", "    path = event['object_uri'][5:]\n    assert path.startswith('warehouse-eu/')\n    total += event['amount'] + len(path)")
        .replace("'preview': event['message'][:160]", "'preview': event['message'][:160], 'path': path");
    let uri_response = i64::try_from(serde_json::to_string(&uri_errors).unwrap().chars().count()).unwrap();
    cases.push(Case::input(
        "json_uri_normalize",
        &uri_source,
        uri_body,
        uri_total + uri_response,
        false,
    ));
    let ndjson = events
        .iter()
        .map(|event| serde_json::to_string(event).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    let error_count = i64::try_from(errors.len()).unwrap();
    for (encoding, input) in [("ascii", ascii_json(&ndjson)), ("utf8", ndjson)] {
        for (parser, source) in [("tail", NDJSON_TAIL), ("splitlines", NDJSON_LINES)] {
            cases.push(Case::input(
                &format!("ndjson_{encoding}_{parser}"),
                source,
                input.clone(),
                total + error_count,
                false,
            ));
        }
    }
    let bulk_ascii = ("x".repeat(159) + "\n").repeat(100_000);
    let bulk_utf8 = ("é".repeat(79) + "x\n").repeat(100_000);
    let bulk_source = "lines = data.splitlines()\ndata = None\nlen(lines) + len(lines[0]) + len(lines[-1])";
    cases.push(Case::input(
        "splitlines_bulk_ascii",
        bulk_source,
        bulk_ascii,
        100_318,
        true,
    ));
    cases.push(Case::input(
        "splitlines_bulk_utf8",
        bulk_source,
        bulk_utf8.clone(),
        100_160,
        true,
    ));
    cases.push(Case::input(
        "splitlines_keep_one",
        "line = data.splitlines()[123]\ndata = None\nlen(line)",
        bulk_utf8,
        80,
        true,
    ));
    cases.push(Case::input(
        "splitlines_inline",
        "lines = data.splitlines()\ndata = None\nlen(lines) + len(lines[-1])",
        "short\n".repeat(100_000),
        100_005,
        true,
    ));
    let document = "Quarterly review: München & Zürich. ".repeat(20_000);
    let preview_body = serde_json::to_string(&json!({"document":document,"request_id":"INV-2026-0042"})).unwrap();
    let code = "import json\nrequest = json.loads(data)\ndata = None\npreview = request['document'][:256]\nrequest = None\nlen(preview)";
    cases.push(Case::input("json_preview_retained", code, preview_body, 256, true));
    cases
}

fn export(cases: &[Case]) {
    let Some(path) = env::var_os("MONTY_SCENARIO_EXPORT_DIR") else {
        return;
    };
    let directory = PathBuf::from(path);
    fs::create_dir_all(&directory).unwrap();
    let mut entries = Vec::new();
    for case in cases {
        let Task::Input(input) = &case.task else { unreachable!() };
        let data = String::try_from(&input.value).unwrap();
        fs::write(directory.join(format!("{}.input", case.name)), &data).unwrap();
        fs::write(directory.join(format!("{}.py", case.name)), &input.source).unwrap();
        entries
            .push(json!({"name":case.name,"expected":input.expected,"input_bytes":data.len(),"retain":input.retain}));
    }
    fs::write(
        directory.join("manifest.json"),
        serde_json::to_string_pretty(&entries).unwrap(),
    )
    .unwrap();
}

#[test]
#[ignore = "application-shaped string workloads; run optimized with --nocapture --test-threads=1"]
fn benchmark_string_realistic_scenarios() {
    let cases = cases();
    export(&cases);
    let policies = vec![
        Policy {
            name: "copy",
            minimum: usize::MAX,
            ratio: 1,
        },
        Policy {
            name: "heuristic",
            minimum: 16,
            ratio: 4,
        },
        Policy {
            name: "heuristic",
            minimum: 128,
            ratio: 4,
        },
        Policy {
            name: "heuristic",
            minimum: 256,
            ratio: 4,
        },
        Policy {
            name: "heuristic",
            minimum: 16,
            ratio: 2,
        },
        Policy {
            name: "view",
            minimum: 16,
            ratio: usize::MAX,
        },
    ];
    run_matrix(cases, policies, 9);
}
