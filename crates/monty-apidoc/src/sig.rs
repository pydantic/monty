//! Reconstructs Rust declarations from rustdoc JSON.
//!
//! rustdoc JSON carries structured types, not source text, so every signature
//! shown on a page is printed from [`rustdoc_types`] data here. Types whose
//! id resolves to a rendered page are wrapped in private marker characters;
//! [`to_plain`] strips the markers for the ```rust fences and
//! [`linked_types`] extracts them for the hidden link payload the docs
//! site's client script applies after syntax highlighting. Unhandled shapes
//! panic with the item name so gaps surface at generation time, not as
//! broken output.

use std::fmt::Write;

use rustdoc_types::{
    Abi, AssocItemConstraintKind, Constant, Crate, DynTrait, Enum, Function, FunctionHeader, FunctionPointer,
    GenericArg, GenericArgs, GenericBound, GenericParamDef, GenericParamDefKind, Generics, Id, Item, ItemEnum,
    MacroKind, Path, PolyTrait, PreciseCapturingArg, ProcMacro, Struct, StructKind, Term, Trait, TraitBoundModifier,
    Type, Variant, VariantKind, WherePredicate,
};

use crate::symbols::SymbolMap;

/// Signatures longer than this wrap one parameter per line, rustfmt-style —
/// the docs sites' content column fits roughly this many monospace
/// characters before a declaration block scrolls horizontally.
const MAX_SIGNATURE_WIDTH: usize = 90;

/// Link markers embedded by [`SigCtx::path_str`]: `\u{1}url\u{2}name\u{3}`.
/// Control characters never appear in Rust source, so they cannot collide.
const LINK_OPEN: char = '\u{1}';
const LINK_SEP: char = '\u{2}';
const LINK_CLOSE: char = '\u{3}';

/// Declaration printer for one crate page: resolves the types it prints
/// against the pages being generated so [`linked_types`] can link them.
pub struct SigCtx<'a> {
    pub krate: &'a Crate,
    pub symbols: &'a SymbolMap,
    /// The crate being rendered, as rustdoc names it (`monty_pool`).
    pub rustdoc_name: &'a str,
}

/// The (name, url) pairs of every link marker in a declaration, deduped in
/// encounter order. Rendered as a hidden `data-links` payload beside the
/// fence; the docs site's client script wraps the matching identifiers in
/// links after syntax highlighting (fenced code cannot carry links, and
/// client-side highlighting would destroy embedded HTML ones).
pub fn linked_types(decl: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut rest = decl;
    while let Some(open) = rest.find(LINK_OPEN) {
        let after = &rest[open + 1..];
        let sep = after.find(LINK_SEP).expect("unterminated link marker");
        let close = after.find(LINK_CLOSE).expect("unterminated link marker");
        let pair = (after[sep + 1..close].to_owned(), after[..sep].to_owned());
        if !out.contains(&pair) {
            out.push(pair);
        }
        rest = &after[close + 1..];
    }
    out
}

/// The declaration with link markers dropped, for prose contexts
/// (the "Implements:" list) and for width measurement.
pub fn to_plain(decl: &str) -> String {
    let mut out = String::with_capacity(decl.len());
    let mut in_url = false;
    for c in decl.chars() {
        match c {
            LINK_OPEN => in_url = true,
            LINK_SEP => in_url = false,
            LINK_CLOSE => {}
            _ if in_url => {}
            _ => out.push(c),
        }
    }
    out
}

impl SigCtx<'_> {
    /// Renders the full declaration block for a rendered item under `name`
    /// (the name it is re-exported as, which may differ from the defining
    /// name). The result carries link markers — finish with [`to_plain`].
    pub fn item_decl(&self, name: &str, item: &Item) -> String {
        match &item.inner {
            ItemEnum::Struct(s) => self.struct_decl(name, s),
            ItemEnum::Enum(e) => self.enum_decl(name, e),
            ItemEnum::Function(f) => format!("{};", self.fn_decl(name, f, "")),
            ItemEnum::Trait(t) => self.trait_decl(name, t),
            ItemEnum::TypeAlias(a) => {
                let (params, where_) = self.generics_parts(&a.generics);
                format!("pub type {name}{params} = {}{where_};", self.type_str(&a.type_))
            }
            ItemEnum::Constant { type_, const_ } => self.const_decl(name, type_, const_),
            ItemEnum::Static(s) => {
                let mut_ = if s.is_mutable { "mut " } else { "" };
                format!("pub static {mut_}{name}: {};", self.type_str(&s.type_))
            }
            ItemEnum::Macro(source) => macro_decl(source),
            ItemEnum::ProcMacro(pm) => proc_macro_decl(name, pm),
            inner => panic!("no declaration renderer for {name}: {:?}", inner.item_kind()),
        }
    }

    /// `pub const NAME: Type = expr;` — falls back to the evaluated value
    /// when rustdoc stringifies the expression as `_`.
    pub fn const_decl(&self, name: &str, type_: &Type, const_: &Constant) -> String {
        let expr = if const_.expr == "_" {
            const_.value.as_deref().unwrap_or("_")
        } else {
            &const_.expr
        };
        format!("pub const {name}: {} = {expr};", self.type_str(type_))
    }

    /// Method/function signature without the trailing `;`, prefixed with
    /// `indent` on every line (used to nest trait items).
    pub fn fn_decl(&self, name: &str, f: &Function, indent: &str) -> String {
        let mut head = format!("{indent}pub {}fn {name}", header_str(&f.header));
        head.push_str(&self.generic_params_str(&f.generics.params));
        let args: Vec<String> = f
            .sig
            .inputs
            .iter()
            .map(|(arg_name, ty)| self.arg_str(arg_name, ty))
            .collect();
        let ret = f
            .sig
            .output
            .as_ref()
            .map(|output| format!(" -> {}", self.type_str(output)))
            .unwrap_or_default();
        let single_line = format!("{head}({}){ret}", args.join(", "));
        let mut out = if args.is_empty() || to_plain(&single_line).len() <= MAX_SIGNATURE_WIDTH {
            single_line
        } else {
            format!(
                "{head}(\n{indent}    {},\n{indent}){ret}",
                args.join(&format!(",\n{indent}    "))
            )
        };
        if let Some(where_) = self.where_str(&f.generics) {
            write!(
                out,
                "\n{indent}where\n{indent}    {}",
                where_.join(&format!(",\n{indent}    "))
            )
            .unwrap();
        }
        out
    }

    /// Prints a type. The workhorse behind every declaration renderer.
    pub fn type_str(&self, ty: &Type) -> String {
        match ty {
            Type::ResolvedPath(path) => self.path_str(path),
            Type::DynTrait(dyn_trait) => self.dyn_trait_str(dyn_trait),
            Type::Generic(name) | Type::Primitive(name) => name.clone(),
            Type::FunctionPointer(fp) => self.function_pointer_str(fp),
            Type::Tuple(items) => {
                let inner: Vec<String> = items.iter().map(|t| self.type_str(t)).collect();
                // one-tuples need the trailing comma to stay one-tuples
                if inner.len() == 1 {
                    format!("({},)", inner[0])
                } else {
                    format!("({})", inner.join(", "))
                }
            }
            Type::Slice(inner) => format!("[{}]", self.type_str(inner)),
            Type::Array { type_, len } => format!("[{}; {len}]", self.type_str(type_)),
            Type::ImplTrait(bounds) => format!("impl {}", self.bounds_str(bounds)),
            Type::Infer => "_".to_owned(),
            Type::RawPointer { is_mutable, type_ } => {
                format!(
                    "*{} {}",
                    if *is_mutable { "mut" } else { "const" },
                    self.type_str(type_)
                )
            }
            Type::BorrowedRef {
                lifetime,
                is_mutable,
                type_,
            } => {
                let lifetime = lifetime.as_ref().map(|l| format!("{l} ")).unwrap_or_default();
                let mut_ = if *is_mutable { "mut " } else { "" };
                format!("&{lifetime}{mut_}{}", self.type_str(type_))
            }
            Type::QualifiedPath {
                name,
                args,
                self_type,
                trait_,
            } => {
                let self_ = self.type_str(self_type);
                let args = args.as_deref().map(|a| self.generic_args_str(a)).unwrap_or_default();
                match trait_ {
                    Some(trait_) => format!("<{self_} as {}>::{name}{args}", self.path_str(trait_)),
                    None => format!("{self_}::{name}{args}"),
                }
            }
            Type::Pat { .. } => panic!("pattern types are unstable and should not appear in the public API"),
        }
    }

    /// Prints a path with its generic arguments, e.g.
    /// `Result<Checkout, PoolError>`. `crate::private_module::Name` paths (as
    /// written in source) collapse to the bare name — the private module
    /// means nothing to a reference reader. Names whose id resolves to a
    /// rendered page are wrapped in link markers.
    pub fn path_str(&self, path: &Path) -> String {
        let args = path
            .args
            .as_deref()
            .map(|a| self.generic_args_str(a))
            .unwrap_or_default();
        let name = if path.path.starts_with("crate::") {
            path.path.rsplit("::").next().expect("empty path")
        } else {
            &path.path
        };
        match self.symbols.resolve(self.rustdoc_name, self.krate, path.id) {
            Some(url) => format!("{LINK_OPEN}{url}{LINK_SEP}{name}{LINK_CLOSE}{args}"),
            None => format!("{name}{args}"),
        }
    }

    /// `<T: Bound, 'a, const N: usize>` — empty when there is nothing to
    /// print. Synthetic params (compiler-introduced for `impl Trait`) are
    /// skipped.
    fn generic_params_str(&self, params: &[GenericParamDef]) -> String {
        let printed: Vec<String> = params
            .iter()
            .filter(|p| !is_synthetic(p))
            .map(|p| self.param_def_str(p))
            .collect();
        if printed.is_empty() {
            String::new()
        } else {
            format!("<{}>", printed.join(", "))
        }
    }

    /// Splits generics into the `<...>` prefix and a joined ` where ...`
    /// suffix for single-line declarations (type aliases).
    fn generics_parts(&self, generics: &Generics) -> (String, String) {
        let params = self.generic_params_str(&generics.params);
        let where_ = match self.where_str(generics) {
            Some(preds) => format!("\nwhere\n    {}", preds.join(",\n    ")),
            None => String::new(),
        };
        (params, where_)
    }

    /// The rendered predicates of a `where` clause, or `None` when there is
    /// none.
    fn where_str(&self, generics: &Generics) -> Option<Vec<String>> {
        let preds: Vec<String> = generics
            .where_predicates
            .iter()
            .map(|p| self.where_predicate_str(p))
            .collect();
        if preds.is_empty() { None } else { Some(preds) }
    }

    fn where_predicate_str(&self, pred: &WherePredicate) -> String {
        match pred {
            WherePredicate::BoundPredicate {
                type_,
                bounds,
                generic_params,
            } => {
                format!(
                    "{}{}: {}",
                    self.hrtb_str(generic_params),
                    self.type_str(type_),
                    self.bounds_str(bounds)
                )
            }
            WherePredicate::LifetimePredicate { lifetime, outlives } => {
                format!("{lifetime}: {}", outlives.join(" + "))
            }
            WherePredicate::EqPredicate { lhs, rhs } => {
                format!("{} = {}", self.type_str(lhs), self.term_str(rhs))
            }
        }
    }

    fn struct_decl(&self, name: &str, s: &Struct) -> String {
        let (params, where_) = self.generics_parts(&s.generics);
        match &s.kind {
            StructKind::Unit => format!("pub struct {name}{params}{where_};"),
            StructKind::Tuple(fields) => {
                let printed: Vec<String> = fields
                    .iter()
                    .map(|field| match field {
                        Some(id) => match &self.krate.index[id].inner {
                            ItemEnum::StructField(ty) => format!("pub {}", self.type_str(ty)),
                            inner => panic!("tuple field of {name} is not a field: {:?}", inner.item_kind()),
                        },
                        None => "/* private */".to_owned(),
                    })
                    .collect();
                format!("pub struct {name}{params}({}){where_};", printed.join(", "))
            }
            StructKind::Plain {
                fields,
                has_stripped_fields,
            } => {
                if fields.is_empty() {
                    // entirely private: show the item exists without inventing a body
                    format!("pub struct {name}{params}{where_} {{ /* private fields */ }}")
                } else {
                    let mut out = format!("pub struct {name}{params}{where_} {{\n");
                    for id in fields {
                        let field = &self.krate.index[id];
                        let ItemEnum::StructField(ty) = &field.inner else {
                            panic!("field of {name} is not a field: {:?}", field.inner.item_kind())
                        };
                        push_doc_lines(&mut out, field, "    ");
                        let field_name = field.name.as_deref().expect("struct field with no name");
                        writeln!(out, "    pub {field_name}: {},", self.type_str(ty)).unwrap();
                    }
                    if *has_stripped_fields {
                        out.push_str("    /* private fields */\n");
                    }
                    out.push('}');
                    out
                }
            }
        }
    }

    fn enum_decl(&self, name: &str, e: &Enum) -> String {
        let (params, where_) = self.generics_parts(&e.generics);
        let mut out = format!("pub enum {name}{params}{where_} {{\n");
        for id in &e.variants {
            let variant = &self.krate.index[id];
            let ItemEnum::Variant(v) = &variant.inner else {
                panic!("variant of {name} is not a variant: {:?}", variant.inner.item_kind())
            };
            push_doc_lines(&mut out, variant, "    ");
            let variant_name = variant.name.as_deref().expect("enum variant with no name");
            write!(out, "    {variant_name}{}", self.variant_body_str(name, v)).unwrap();
            if let Some(discriminant) = &v.discriminant {
                write!(out, " = {}", discriminant.expr).unwrap();
            }
            out.push_str(",\n");
        }
        if e.has_stripped_variants {
            out.push_str("    /* hidden variants */\n");
        }
        out.push('}');
        out
    }

    /// The `(T, U)` or `{ field: T }` payload of one enum variant.
    fn variant_body_str(&self, enum_name: &str, v: &Variant) -> String {
        let field_type = |id: &Id| match &self.krate.index[id].inner {
            ItemEnum::StructField(ty) => self.type_str(ty),
            inner => panic!("variant field of {enum_name} is not a field: {:?}", inner.item_kind()),
        };
        match &v.kind {
            VariantKind::Plain => String::new(),
            VariantKind::Tuple(fields) => {
                let printed: Vec<String> = fields
                    .iter()
                    .map(|field| field.as_ref().map_or_else(|| "/* private */".to_owned(), field_type))
                    .collect();
                format!("({})", printed.join(", "))
            }
            VariantKind::Struct {
                fields,
                has_stripped_fields,
            } => {
                let mut printed: Vec<String> = fields
                    .iter()
                    .map(|id| {
                        let field = &self.krate.index[id];
                        format!(
                            "{}: {}",
                            field.name.as_deref().expect("variant field with no name"),
                            field_type(id)
                        )
                    })
                    .collect();
                if *has_stripped_fields {
                    printed.push("/* private fields */".to_owned());
                }
                format!(" {{ {} }}", printed.join(", "))
            }
        }
    }

    /// Trait declaration with its associated items' signatures in the body;
    /// item docs are rendered as prose by the caller, not repeated here.
    fn trait_decl(&self, name: &str, t: &Trait) -> String {
        let unsafe_ = if t.is_unsafe { "unsafe " } else { "" };
        let params = self.generic_params_str(&t.generics.params);
        let bounds = if t.bounds.is_empty() {
            String::new()
        } else {
            format!(": {}", self.bounds_str(&t.bounds))
        };
        let where_ = match self.where_str(&t.generics) {
            Some(preds) => format!("\nwhere\n    {}", preds.join(",\n    ")),
            None => String::new(),
        };
        let mut out = format!("pub {unsafe_}trait {name}{params}{bounds}{where_} {{\n");
        for id in &t.items {
            let assoc = &self.krate.index[id];
            let assoc_name = assoc.name.as_deref().expect("trait item with no name");
            match &assoc.inner {
                ItemEnum::Function(f) => {
                    // trait methods have no `pub`; provided methods get `{ ... }`
                    let decl = self.fn_decl(assoc_name, f, "    ").replacen("pub ", "", 1);
                    let terminator = if f.has_body { " { ... }" } else { ";" };
                    writeln!(out, "{decl}{terminator}").unwrap();
                }
                ItemEnum::AssocConst { type_, value, .. } => {
                    let default = value.as_ref().map(|v| format!(" = {v}")).unwrap_or_default();
                    writeln!(out, "    const {assoc_name}: {}{default};", self.type_str(type_)).unwrap();
                }
                ItemEnum::AssocType { bounds, type_, .. } => {
                    let bounds = if bounds.is_empty() {
                        String::new()
                    } else {
                        format!(": {}", self.bounds_str(bounds))
                    };
                    let default = type_
                        .as_ref()
                        .map(|t| format!(" = {}", self.type_str(t)))
                        .unwrap_or_default();
                    writeln!(out, "    type {assoc_name}{bounds}{default};").unwrap();
                }
                inner => panic!("unhandled trait item in {name}: {:?}", inner.item_kind()),
            }
        }
        out.push('}');
        out
    }

    /// One function argument; `self` receivers get their idiomatic shorthand.
    fn arg_str(&self, name: &str, ty: &Type) -> String {
        if name == "self" {
            match ty {
                Type::Generic(self_) if self_ == "Self" => return "self".to_owned(),
                Type::BorrowedRef {
                    lifetime,
                    is_mutable,
                    type_,
                } => {
                    if matches!(&**type_, Type::Generic(self_) if self_ == "Self") {
                        let lifetime = lifetime.as_ref().map(|l| format!("{l} ")).unwrap_or_default();
                        let mut_ = if *is_mutable { "mut " } else { "" };
                        return format!("&{lifetime}{mut_}self");
                    }
                }
                _ => {}
            }
            return format!("self: {}", self.type_str(ty));
        }
        format!("{name}: {}", self.type_str(ty))
    }

    fn generic_args_str(&self, args: &GenericArgs) -> String {
        match args {
            GenericArgs::AngleBracketed { args, constraints } => {
                let mut printed: Vec<String> = args.iter().map(|a| self.generic_arg_str(a)).collect();
                for constraint in constraints {
                    let args = constraint
                        .args
                        .as_deref()
                        .map(|a| self.generic_args_str(a))
                        .unwrap_or_default();
                    let binding = match &constraint.binding {
                        AssocItemConstraintKind::Equality(term) => format!(" = {}", self.term_str(term)),
                        AssocItemConstraintKind::Constraint(bounds) => format!(": {}", self.bounds_str(bounds)),
                    };
                    printed.push(format!("{}{args}{binding}", constraint.name));
                }
                if printed.is_empty() {
                    String::new()
                } else {
                    format!("<{}>", printed.join(", "))
                }
            }
            GenericArgs::Parenthesized { inputs, output } => {
                let inputs: Vec<String> = inputs.iter().map(|t| self.type_str(t)).collect();
                let output = output
                    .as_ref()
                    .map(|t| format!(" -> {}", self.type_str(t)))
                    .unwrap_or_default();
                format!("({}){output}", inputs.join(", "))
            }
            GenericArgs::ReturnTypeNotation => "(..)".to_owned(),
        }
    }

    fn generic_arg_str(&self, arg: &GenericArg) -> String {
        match arg {
            GenericArg::Lifetime(lifetime) => lifetime.clone(),
            GenericArg::Type(ty) => self.type_str(ty),
            GenericArg::Const(constant) => constant.expr.clone(),
            GenericArg::Infer => "_".to_owned(),
        }
    }

    fn term_str(&self, term: &Term) -> String {
        match term {
            Term::Type(ty) => self.type_str(ty),
            Term::Constant(constant) => constant.expr.clone(),
        }
    }

    /// ` + `-joined bounds, e.g. `FnMut(PrintEvent) + Send + 'static`.
    fn bounds_str(&self, bounds: &[GenericBound]) -> String {
        let printed: Vec<String> = bounds.iter().map(|b| self.bound_str(b)).collect();
        printed.join(" + ")
    }

    fn bound_str(&self, bound: &GenericBound) -> String {
        match bound {
            GenericBound::TraitBound {
                trait_,
                generic_params,
                modifier,
            } => {
                let modifier = match modifier {
                    TraitBoundModifier::None => "",
                    TraitBoundModifier::Maybe => "?",
                    TraitBoundModifier::MaybeConst => "~const ",
                };
                format!("{}{modifier}{}", self.hrtb_str(generic_params), self.path_str(trait_))
            }
            GenericBound::Outlives(lifetime) => lifetime.clone(),
            GenericBound::Use(args) => {
                let printed: Vec<&str> = args
                    .iter()
                    .map(|arg| match arg {
                        PreciseCapturingArg::Lifetime(name) | PreciseCapturingArg::Param(name) => name.as_str(),
                    })
                    .collect();
                format!("use<{}>", printed.join(", "))
            }
        }
    }

    /// `for<'a> ` prefix for higher-ranked trait bounds, empty when not
    /// needed.
    fn hrtb_str(&self, generic_params: &[GenericParamDef]) -> String {
        if generic_params.is_empty() {
            String::new()
        } else {
            let printed: Vec<String> = generic_params.iter().map(|p| self.param_def_str(p)).collect();
            format!("for<{}> ", printed.join(", "))
        }
    }

    fn param_def_str(&self, param: &GenericParamDef) -> String {
        match &param.kind {
            GenericParamDefKind::Lifetime { outlives } => {
                if outlives.is_empty() {
                    param.name.clone()
                } else {
                    format!("{}: {}", param.name, outlives.join(" + "))
                }
            }
            GenericParamDefKind::Type { bounds, default, .. } => {
                let mut out = param.name.clone();
                if !bounds.is_empty() {
                    write!(out, ": {}", self.bounds_str(bounds)).unwrap();
                }
                if let Some(default) = default {
                    write!(out, " = {}", self.type_str(default)).unwrap();
                }
                out
            }
            GenericParamDefKind::Const { type_, default } => {
                let default = default.as_ref().map(|d| format!(" = {d}")).unwrap_or_default();
                format!("const {}: {}{default}", param.name, self.type_str(type_))
            }
        }
    }

    fn dyn_trait_str(&self, dyn_trait: &DynTrait) -> String {
        let mut parts: Vec<String> = dyn_trait.traits.iter().map(|p| self.poly_trait_str(p)).collect();
        if let Some(lifetime) = &dyn_trait.lifetime {
            parts.push(lifetime.clone());
        }
        format!("dyn {}", parts.join(" + "))
    }

    fn poly_trait_str(&self, poly: &PolyTrait) -> String {
        format!("{}{}", self.hrtb_str(&poly.generic_params), self.path_str(&poly.trait_))
    }

    fn function_pointer_str(&self, fp: &FunctionPointer) -> String {
        let args: Vec<String> = fp.sig.inputs.iter().map(|(name, ty)| self.arg_str(name, ty)).collect();
        let output = fp
            .sig
            .output
            .as_ref()
            .map(|t| format!(" -> {}", self.type_str(t)))
            .unwrap_or_default();
        format!(
            "{}{}fn({}){output}",
            self.hrtb_str(&fp.generic_params),
            header_str(&fp.header),
            args.join(", ")
        )
    }
}

/// Proc macros render as their use-site form, with derive helper attributes
/// noted alongside.
fn proc_macro_decl(name: &str, pm: &ProcMacro) -> String {
    match pm.kind {
        MacroKind::Derive => {
            let mut out = format!("#[derive({name})]");
            if !pm.helpers.is_empty() {
                let helpers: Vec<String> = pm.helpers.iter().map(|h| format!("#[{h}(...)]")).collect();
                write!(out, "\n// helper attributes: {}", helpers.join(", ")).unwrap();
            }
            out
        }
        MacroKind::Attr => format!("#[{name}]"),
        MacroKind::Bang => format!("{name}!()"),
    }
}

/// `macro_rules!` bodies are noise on a reference page — keep the name only.
fn macro_decl(source: &str) -> String {
    let head = source.split('{').next().unwrap_or(source).trim_end();
    format!("{head} {{ /* macro body */ }}")
}

/// Appends an item's doc comment as indented `///` lines inside a decl block
/// (used for struct fields and enum variants, whose docs read best in place).
fn push_doc_lines(out: &mut String, item: &Item, indent: &str) {
    if let Some(docs) = &item.docs {
        for line in docs.lines() {
            let space = if line.is_empty() { "" } else { " " };
            writeln!(out, "{indent}///{space}{line}").unwrap();
        }
    }
}

fn header_str(header: &FunctionHeader) -> String {
    let mut out = String::new();
    if header.is_const {
        out.push_str("const ");
    }
    if header.is_async {
        out.push_str("async ");
    }
    if header.is_unsafe {
        out.push_str("unsafe ");
    }
    if !matches!(header.abi, Abi::Rust) {
        out.push_str("extern \"C\" ");
    }
    out
}

fn is_synthetic(param: &GenericParamDef) -> bool {
    matches!(&param.kind, GenericParamDefKind::Type { is_synthetic: true, .. })
}
