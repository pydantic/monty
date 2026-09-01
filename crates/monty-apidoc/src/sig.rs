//! Reconstructs Rust declarations from rustdoc JSON for ```rust fences.
//!
//! rustdoc JSON carries structured types, not source text, so every signature
//! shown on a page is printed from [`rustdoc_types`] data here. Unhandled
//! shapes panic with the item name so gaps surface at generation time rather
//! than as broken output.

use std::fmt::Write;

use rustdoc_types::{
    Abi, AssocItemConstraintKind, Constant, Crate, DynTrait, Enum, Function, FunctionHeader, FunctionPointer,
    GenericArg, GenericArgs, GenericBound, GenericParamDef, GenericParamDefKind, Generics, Id, Item, ItemEnum, Path,
    PolyTrait, PreciseCapturingArg, Struct, StructKind, Term, Trait, TraitBoundModifier, Type, Variant, VariantKind,
    WherePredicate,
};

/// Renders the full declaration block for a rendered item under `name` (the
/// name it is re-exported as, which may differ from the defining name).
pub fn item_decl(name: &str, item: &Item, krate: &Crate) -> String {
    match &item.inner {
        ItemEnum::Struct(s) => struct_decl(name, s, krate),
        ItemEnum::Enum(e) => enum_decl(name, e, krate),
        ItemEnum::Function(f) => format!("{};", fn_decl(name, f, "")),
        ItemEnum::Trait(t) => trait_decl(name, t, krate),
        ItemEnum::TypeAlias(a) => {
            let (params, where_) = generics_parts(&a.generics);
            format!("pub type {name}{params} = {}{where_};", type_str(&a.type_))
        }
        ItemEnum::Constant { type_, const_ } => const_decl(name, type_, const_),
        ItemEnum::Static(s) => {
            let mut_ = if s.is_mutable { "mut " } else { "" };
            format!("pub static {mut_}{name}: {};", type_str(&s.type_))
        }
        ItemEnum::Macro(source) => macro_decl(source),
        inner => panic!("no declaration renderer for {name}: {:?}", inner.item_kind()),
    }
}

/// `pub const NAME: Type = expr;` — falls back to the evaluated value when
/// rustdoc stringifies the expression as `_`.
pub fn const_decl(name: &str, type_: &Type, const_: &Constant) -> String {
    let expr = if const_.expr == "_" {
        const_.value.as_deref().unwrap_or("_")
    } else {
        &const_.expr
    };
    format!("pub const {name}: {} = {expr};", type_str(type_))
}

/// Method/function signature without the trailing `;`, prefixed with `indent`
/// on every line (used to nest trait items).
pub fn fn_decl(name: &str, f: &Function, indent: &str) -> String {
    let mut out = format!("{indent}pub {}fn {name}", header_str(&f.header));
    out.push_str(&generic_params_str(&f.generics.params));
    out.push('(');
    let args: Vec<String> = f
        .sig
        .inputs
        .iter()
        .map(|(arg_name, ty)| arg_str(arg_name, ty))
        .collect();
    out.push_str(&args.join(", "));
    out.push(')');
    if let Some(output) = &f.sig.output {
        write!(out, " -> {}", type_str(output)).unwrap();
    }
    if let Some(where_) = where_str(&f.generics) {
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
pub fn type_str(ty: &Type) -> String {
    match ty {
        Type::ResolvedPath(path) => path_str(path),
        Type::DynTrait(dyn_trait) => dyn_trait_str(dyn_trait),
        Type::Generic(name) | Type::Primitive(name) => name.clone(),
        Type::FunctionPointer(fp) => function_pointer_str(fp),
        Type::Tuple(items) => {
            let inner: Vec<String> = items.iter().map(type_str).collect();
            // one-tuples need the trailing comma to stay one-tuples
            if inner.len() == 1 {
                format!("({},)", inner[0])
            } else {
                format!("({})", inner.join(", "))
            }
        }
        Type::Slice(inner) => format!("[{}]", type_str(inner)),
        Type::Array { type_, len } => format!("[{}; {len}]", type_str(type_)),
        Type::ImplTrait(bounds) => format!("impl {}", bounds_str(bounds)),
        Type::Infer => "_".to_owned(),
        Type::RawPointer { is_mutable, type_ } => {
            format!("*{} {}", if *is_mutable { "mut" } else { "const" }, type_str(type_))
        }
        Type::BorrowedRef {
            lifetime,
            is_mutable,
            type_,
        } => {
            let lifetime = lifetime.as_ref().map(|l| format!("{l} ")).unwrap_or_default();
            let mut_ = if *is_mutable { "mut " } else { "" };
            format!("&{lifetime}{mut_}{}", type_str(type_))
        }
        Type::QualifiedPath {
            name,
            args,
            self_type,
            trait_,
        } => {
            let self_ = type_str(self_type);
            let args = args.as_deref().map(generic_args_str).unwrap_or_default();
            match trait_ {
                Some(trait_) => format!("<{self_} as {}>::{name}{args}", path_str(trait_)),
                None => format!("{self_}::{name}{args}"),
            }
        }
        Type::Pat { .. } => panic!("pattern types are unstable and should not appear in the public API"),
    }
}

/// `<T: Bound, 'a, const N: usize>` — empty string when there is nothing to
/// print. Synthetic params (compiler-introduced for `impl Trait`) are skipped.
pub fn generic_params_str(params: &[GenericParamDef]) -> String {
    let printed: Vec<String> = params.iter().filter(|p| !is_synthetic(p)).map(param_def_str).collect();
    if printed.is_empty() {
        String::new()
    } else {
        format!("<{}>", printed.join(", "))
    }
}

/// Splits generics into the `<...>` prefix and a joined ` where ...` suffix
/// for single-line declarations (type aliases).
fn generics_parts(generics: &Generics) -> (String, String) {
    let params = generic_params_str(&generics.params);
    let where_ = match where_str(generics) {
        Some(preds) => format!("\nwhere\n    {}", preds.join(",\n    ")),
        None => String::new(),
    };
    (params, where_)
}

/// The rendered predicates of a `where` clause, or `None` when there is none.
fn where_str(generics: &Generics) -> Option<Vec<String>> {
    let preds: Vec<String> = generics.where_predicates.iter().map(where_predicate_str).collect();
    if preds.is_empty() { None } else { Some(preds) }
}

fn where_predicate_str(pred: &WherePredicate) -> String {
    match pred {
        WherePredicate::BoundPredicate {
            type_,
            bounds,
            generic_params,
        } => {
            format!(
                "{}{}: {}",
                hrtb_str(generic_params),
                type_str(type_),
                bounds_str(bounds)
            )
        }
        WherePredicate::LifetimePredicate { lifetime, outlives } => {
            format!("{lifetime}: {}", outlives.join(" + "))
        }
        WherePredicate::EqPredicate { lhs, rhs } => format!("{} = {}", type_str(lhs), term_str(rhs)),
    }
}

fn struct_decl(name: &str, s: &Struct, krate: &Crate) -> String {
    let (params, where_) = generics_parts(&s.generics);
    match &s.kind {
        StructKind::Unit => format!("pub struct {name}{params}{where_};"),
        StructKind::Tuple(fields) => {
            let printed: Vec<String> = fields
                .iter()
                .map(|field| match field {
                    Some(id) => match &krate.index[id].inner {
                        ItemEnum::StructField(ty) => format!("pub {}", type_str(ty)),
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
                    let field = &krate.index[id];
                    let ItemEnum::StructField(ty) = &field.inner else {
                        panic!("field of {name} is not a field: {:?}", field.inner.item_kind())
                    };
                    push_doc_lines(&mut out, field, "    ");
                    let field_name = field.name.as_deref().expect("struct field with no name");
                    writeln!(out, "    pub {field_name}: {},", type_str(ty)).unwrap();
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

fn enum_decl(name: &str, e: &Enum, krate: &Crate) -> String {
    let (params, where_) = generics_parts(&e.generics);
    let mut out = format!("pub enum {name}{params}{where_} {{\n");
    for id in &e.variants {
        let variant = &krate.index[id];
        let ItemEnum::Variant(v) = &variant.inner else {
            panic!("variant of {name} is not a variant: {:?}", variant.inner.item_kind())
        };
        push_doc_lines(&mut out, variant, "    ");
        let variant_name = variant.name.as_deref().expect("enum variant with no name");
        write!(out, "    {variant_name}{}", variant_body_str(name, v, krate)).unwrap();
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
fn variant_body_str(enum_name: &str, v: &Variant, krate: &Crate) -> String {
    let field_type = |id: &Id| match &krate.index[id].inner {
        ItemEnum::StructField(ty) => type_str(ty),
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
                    let field = &krate.index[id];
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

/// Trait declaration with its associated items' signatures in the body; item
/// docs are rendered as prose by the caller, not repeated here.
fn trait_decl(name: &str, t: &Trait, krate: &Crate) -> String {
    let unsafe_ = if t.is_unsafe { "unsafe " } else { "" };
    let params = generic_params_str(&t.generics.params);
    let bounds = if t.bounds.is_empty() {
        String::new()
    } else {
        format!(": {}", bounds_str(&t.bounds))
    };
    let where_ = match where_str(&t.generics) {
        Some(preds) => format!("\nwhere\n    {}", preds.join(",\n    ")),
        None => String::new(),
    };
    let mut out = format!("pub {unsafe_}trait {name}{params}{bounds}{where_} {{\n");
    for id in &t.items {
        let assoc = &krate.index[id];
        let assoc_name = assoc.name.as_deref().expect("trait item with no name");
        match &assoc.inner {
            ItemEnum::Function(f) => {
                // trait methods have no `pub`; provided methods get `{ ... }`
                let decl = fn_decl(assoc_name, f, "    ").replacen("pub ", "", 1);
                let terminator = if f.has_body { " { ... }" } else { ";" };
                writeln!(out, "{decl}{terminator}").unwrap();
            }
            ItemEnum::AssocConst { type_, value, .. } => {
                let default = value.as_ref().map(|v| format!(" = {v}")).unwrap_or_default();
                writeln!(out, "    const {assoc_name}: {}{default};", type_str(type_)).unwrap();
            }
            ItemEnum::AssocType { bounds, type_, .. } => {
                let bounds = if bounds.is_empty() {
                    String::new()
                } else {
                    format!(": {}", bounds_str(bounds))
                };
                let default = type_
                    .as_ref()
                    .map(|t| format!(" = {}", type_str(t)))
                    .unwrap_or_default();
                writeln!(out, "    type {assoc_name}{bounds}{default};").unwrap();
            }
            inner => panic!("unhandled trait item in {name}: {:?}", inner.item_kind()),
        }
    }
    out.push('}');
    out
}

/// `macro_rules!` bodies are noise on a reference page — keep the name only.
fn macro_decl(source: &str) -> String {
    let head = source.split('{').next().unwrap_or(source).trim_end();
    format!("{head} {{ /* macro body */ }}")
}

/// Appends an item's doc comment as indented `///` lines inside a decl fence
/// (used for struct fields and enum variants, whose docs read best in place).
fn push_doc_lines(out: &mut String, item: &Item, indent: &str) {
    if let Some(docs) = &item.docs {
        for line in docs.lines() {
            let space = if line.is_empty() { "" } else { " " };
            writeln!(out, "{indent}///{space}{line}").unwrap();
        }
    }
}

/// One function argument; `self` receivers get their idiomatic shorthand.
fn arg_str(name: &str, ty: &Type) -> String {
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
        return format!("self: {}", type_str(ty));
    }
    format!("{name}: {}", type_str(ty))
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

/// Prints a path with its generic arguments, e.g. `Result<Checkout, PoolError>`.
/// `crate::private_module::Name` paths (as written in source) collapse to the
/// bare name — the private module means nothing to a reference reader.
pub fn path_str(path: &Path) -> String {
    let args = path.args.as_deref().map(generic_args_str).unwrap_or_default();
    let name = if path.path.starts_with("crate::") {
        path.path.rsplit("::").next().expect("empty path")
    } else {
        &path.path
    };
    format!("{name}{args}")
}

fn generic_args_str(args: &GenericArgs) -> String {
    match args {
        GenericArgs::AngleBracketed { args, constraints } => {
            let mut printed: Vec<String> = args.iter().map(generic_arg_str).collect();
            for constraint in constraints {
                let args = constraint.args.as_deref().map(generic_args_str).unwrap_or_default();
                let binding = match &constraint.binding {
                    AssocItemConstraintKind::Equality(term) => format!(" = {}", term_str(term)),
                    AssocItemConstraintKind::Constraint(bounds) => format!(": {}", bounds_str(bounds)),
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
            let inputs: Vec<String> = inputs.iter().map(type_str).collect();
            let output = output
                .as_ref()
                .map(|t| format!(" -> {}", type_str(t)))
                .unwrap_or_default();
            format!("({}){output}", inputs.join(", "))
        }
        GenericArgs::ReturnTypeNotation => "(..)".to_owned(),
    }
}

fn generic_arg_str(arg: &GenericArg) -> String {
    match arg {
        GenericArg::Lifetime(lifetime) => lifetime.clone(),
        GenericArg::Type(ty) => type_str(ty),
        GenericArg::Const(constant) => constant.expr.clone(),
        GenericArg::Infer => "_".to_owned(),
    }
}

fn term_str(term: &Term) -> String {
    match term {
        Term::Type(ty) => type_str(ty),
        Term::Constant(constant) => constant.expr.clone(),
    }
}

/// ` + `-joined bounds, e.g. `FnMut(PrintEvent) + Send + 'static`.
fn bounds_str(bounds: &[GenericBound]) -> String {
    let printed: Vec<String> = bounds.iter().map(bound_str).collect();
    printed.join(" + ")
}

fn bound_str(bound: &GenericBound) -> String {
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
            format!("{}{modifier}{}", hrtb_str(generic_params), path_str(trait_))
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

/// `for<'a> ` prefix for higher-ranked trait bounds, empty when not needed.
fn hrtb_str(generic_params: &[GenericParamDef]) -> String {
    if generic_params.is_empty() {
        String::new()
    } else {
        let printed: Vec<String> = generic_params.iter().map(param_def_str).collect();
        format!("for<{}> ", printed.join(", "))
    }
}

fn param_def_str(param: &GenericParamDef) -> String {
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
                write!(out, ": {}", bounds_str(bounds)).unwrap();
            }
            if let Some(default) = default {
                write!(out, " = {}", type_str(default)).unwrap();
            }
            out
        }
        GenericParamDefKind::Const { type_, default } => {
            let default = default.as_ref().map(|d| format!(" = {d}")).unwrap_or_default();
            format!("const {}: {}{default}", param.name, type_str(type_))
        }
    }
}

fn is_synthetic(param: &GenericParamDef) -> bool {
    matches!(&param.kind, GenericParamDefKind::Type { is_synthetic: true, .. })
}

fn dyn_trait_str(dyn_trait: &DynTrait) -> String {
    let mut parts: Vec<String> = dyn_trait.traits.iter().map(poly_trait_str).collect();
    if let Some(lifetime) = &dyn_trait.lifetime {
        parts.push(lifetime.clone());
    }
    format!("dyn {}", parts.join(" + "))
}

fn poly_trait_str(poly: &PolyTrait) -> String {
    format!("{}{}", hrtb_str(&poly.generic_params), path_str(&poly.trait_))
}

fn function_pointer_str(fp: &FunctionPointer) -> String {
    let args: Vec<String> = fp.sig.inputs.iter().map(|(name, ty)| arg_str(name, ty)).collect();
    let output = fp
        .sig
        .output
        .as_ref()
        .map(|t| format!(" -> {}", type_str(t)))
        .unwrap_or_default();
    format!(
        "{}{}fn({}){output}",
        hrtb_str(&fp.generic_params),
        header_str(&fp.header),
        args.join(", ")
    )
}
