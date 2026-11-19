//! Numeric identities captured before replacing Postcard's identity encoding.

use std::fmt::Write;

use insta::assert_snapshot;
use monty::MontyRun;
use monty_types::{BuiltinsFunctions, CompileOptions, ExcType, MontyObject, MontyType};
use strum::VariantNames;

/// Pins builtin, module-function and marker IDs independently of their encoder.
#[test]
fn inline_identities_keep_legacy_numbers() {
    let names = BuiltinsFunctions::VARIANTS
        .iter()
        .copied()
        // Attribute access binds this hook to a fresh heap method, not an inline builtin.
        .filter(|name| *name != "object.__init_subclass__")
        .chain(ExcType::VARIANTS.iter().copied())
        .chain([
            "bool",
            "int",
            "float",
            "complex",
            "range",
            "slice",
            "str",
            "bytes",
            "list",
            "tuple",
            "dict",
            "set",
            "frozenset",
            "object",
            "type(None)",
            "type(Ellipsis)",
            "type(NotImplemented)",
            "type(len)",
            "type(lambda: None)",
            "type(iter([]))",
            "type(iter(()))",
            "type(iter(range(1)))",
            "type(iter({}))",
            "type(iter(b''))",
            "type(iter(''))",
            "datetime.date",
            "datetime.datetime",
            "datetime.time",
            "datetime.timedelta",
            "datetime.timezone",
            "pathlib.Path",
            "collections.deque",
            "collections.defaultdict",
            "collections.Counter",
            "functools.partial",
            "random.Random",
            "decimal.Decimal",
            "decimal.Context",
            "annotationlib.ForwardRef",
            "asyncio.run",
            "asyncio.gather",
            "collections.namedtuple",
            "json.loads",
            "json.dumps",
            "math.floor",
            "math.fma",
            "os.getenv",
            "os.walk",
            "os.path.join",
            "os.path.samefile",
            "re.compile",
            "re.escape",
            "unicodedata.name",
            "unicodedata.lookup",
            "itertools.chain",
            "itertools.product",
            "itertools.tee",
            "itertools.chain.from_iterable",
            "dataclasses.dataclass",
            "dataclasses.is_dataclass",
            "dataclasses.field",
            "dataclasses.dataclass(eq=False, frozen=False)",
            "dataclasses.dataclass(eq=True, frozen=False)",
            "dataclasses.dataclass(eq=False, frozen=True)",
            "dataclasses.dataclass(eq=True, frozen=True)",
            "functools.reduce",
            "functools.cache",
            "functools.lru_cache",
            "base64.b64encode",
            "binascii.hexlify",
            "random.random",
            "random.binomialvariate",
            "copy.copy",
            "copy.deepcopy",
            "time.time",
            "time.strftime",
            "hashlib.new",
            "hashlib.sha256",
            "bisect.bisect_left",
            "csv.reader",
            "tomllib.loads",
            "decimal.getcontext",
            "decimal.localcontext",
            "fnmatch.fnmatch",
            "typing.get_type_hints",
            "annotationlib.get_annotations",
            "statistics.mean",
            "statistics.quantiles",
            "sys.stdout",
            "sys.stderr",
            "dataclasses.MISSING",
            "typing.Any",
            "typing.Optional",
            "typing.ClassVar",
            "typing.Annotated",
            "typing.Generator",
            "typing.TYPE_CHECKING",
        ]);
    let mut code = String::from(
        "import annotationlib, asyncio, base64, binascii, bisect, collections, copy, csv, dataclasses, datetime, decimal, fnmatch, functools, hashlib, itertools, json, math, os, pathlib, random, re, statistics, sys, time, tomllib, typing, unicodedata\nimport csv as _csv\nresults = []\n",
    );
    for name in names {
        let expression = match name {
            "FrozenInstanceError" => "dataclasses.FrozenInstanceError",
            "io.UnsupportedOperation" => "unsupported_operation",
            _ => name,
        };
        writeln!(code, "results.append({name:?} + ': ' + str(id({expression})))").unwrap();
    }
    code.push_str("'\\n'.join(results)");
    // `io` is not importable; supply its exception class through the host boundary.
    let result = MontyRun::new(
        code,
        "identity.py",
        vec!["unsupported_operation".to_owned()],
        CompileOptions::default(),
    )
    .unwrap()
    .run_no_limits(vec![MontyObject::type_object(MontyType::Exception(
        ExcType::UnsupportedOperation,
    ))])
    .unwrap();
    assert_snapshot!(result.to_string());
}

/// Test-only module functions retain the indices following production modules.
#[cfg(feature = "test-hooks")]
#[test]
fn test_hook_identities_keep_legacy_numbers() {
    let result = MontyRun::new(
        "import gc, sys\n[id(gc.collect), id(gc.disable), id(gc.enable), id(sys.setrecursionlimit)]".to_owned(),
        "identity.py",
        vec![],
        CompileOptions::default(),
    )
    .unwrap()
    .run_no_limits(vec![])
    .unwrap();
    assert_snapshot!(result.to_string());
}
