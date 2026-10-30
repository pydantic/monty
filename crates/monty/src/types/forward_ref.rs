//! `annotationlib.ForwardRef` — an annotation kept as source text.
//!
//! A `ForwardRef` is what `typing.get_type_hints(format=Format.FORWARDREF)`
//! returns for a name it cannot resolve. While such an evaluation runs, every
//! `ForwardRef` doubles as CPython's `_Stringifier`: an unresolved name loads
//! as one, and subscripting, attribute access, calls and operators on it build
//! a new `ForwardRef` holding the composed source text, so `Later | None`
//! becomes `ForwardRef('Later | None')`. Outside such an evaluation the object
//! has the plain `ForwardRef` behaviour: equality by text, `|` builds a union,
//! and `evaluate()` compiles the text.

use std::{
    fmt::Write,
    hash::{Hash, Hasher},
};

use ahash::AHasher;
use serde::{Deserialize, Serialize};

use crate::{
    args::{ArgValues, FromArgs},
    builtins::{Builtins, BuiltinsFunctions},
    bytecode::{CallResult, FrameGlobals, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult, SimpleException},
    frozen::{FrozenFunction, FrozenFunctions},
    hash::{HashValue, identity_hash},
    heap::{ContainsHeap, DropWithContext, HeapData, HeapId, HeapItem, HeapObjectRead, HeapReadOutput},
    intern::StaticStrings,
    modules::{ModuleFunctions, typing},
    types::{
        BuiltinType, LazyHeapSet, PyTrait, Type, Union,
        generic_alias::repr_type_arg,
        str::{StringRepr, allocate_string},
    },
    value::{EitherStr, VALUE_SIZE, Value},
};

/// `ForwardRef.evaluate()`: the plain `VALUE` evaluation raises as `eval()` would;
/// `FORWARDREF` returns the reference itself when the text cannot be evaluated or
/// is a bare name that stays unresolved, otherwise what the text evaluates to with
/// unresolved names as new references.
pub(crate) const EVALUATE_SOURCE: &str = r"
def __monty_forward_ref_evaluate(self, text, globals_, locals_, owner, is_class, forwardref, eval_hint, forward_ref, isinstance_, exception, syntax_error):
    if not forwardref:
        return eval_hint(text, globals_, locals_, False, True, 0, None, False)
    try:
        value = eval_hint(text, globals_, locals_, False, True, 1, owner, is_class)
    except syntax_error:
        raise
    except exception:
        return self
    if isinstance_(value, forward_ref) and text.isidentifier():
        return self
    return value
";

/// How tightly a composed text binds, from `ast.unparse`'s precedence table, so
/// embedding it in a larger expression parenthesizes `(Later | None)[int]` but
/// not `Later[int] | None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) enum Precedence {
    /// Arbitrary text of unknown shape, parenthesized wherever it is embedded.
    Tuple,
    /// What a call argument or subscript item demands: nothing is parenthesized.
    Test,
    BitOr,
    BitXor,
    BitAnd,
    Shift,
    Arith,
    Term,
    /// A unary `-x`, `+x` or `~x`.
    Factor,
    Power,
    Await,
    /// A name, attribute, subscript or call.
    Atom,
}

impl Precedence {
    /// The next tighter level, which the non-associative side of an operator demands.
    fn next(self) -> Self {
        match self {
            Self::Tuple => Self::Test,
            Self::Test => Self::BitOr,
            Self::BitOr => Self::BitXor,
            Self::BitXor => Self::BitAnd,
            Self::BitAnd => Self::Shift,
            Self::Shift => Self::Arith,
            Self::Arith => Self::Term,
            Self::Term => Self::Factor,
            Self::Factor => Self::Power,
            Self::Power => Self::Await,
            Self::Await | Self::Atom => Self::Atom,
        }
    }
}

/// An `annotationlib.ForwardRef`.
///
/// `arg`, `module` and `owner` are OWNED values (`arg` a `str`, `module` a `str`
/// or `None`, `owner` the function or class the annotation belongs to, or `None`),
/// as is a `globals` dict, so `py_dec_ref_ids` and `for_each_child_id` must
/// release and trace all four.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ForwardRef {
    /// `__forward_arg__`, the annotation source.
    arg: Value,
    /// `__forward_module__`.
    module: Value,
    /// `__owner__`.
    owner: Value,
    /// `__forward_is_argument__`, compared by nothing but kept for introspection.
    is_argument: bool,
    /// `__forward_is_class__`: the annotation was written in a class body.
    is_class: bool,
    /// How tightly [`Self::arg`] binds when composed into a larger text.
    precedence: Precedence,
    /// The globals a stringifier was composed against (CPython's `__globals__`),
    /// part of equality by identity; `None` for a constructed reference.
    globals: Option<FrameGlobals>,
}

/// Arguments of `ForwardRef(arg, *, module=None, owner=None, is_argument=True, is_class=False)`.
#[derive(FromArgs)]
#[from_args(name = "ForwardRef.__init__", style = def)]
struct ForwardRefArgs {
    #[from_args(static_string = "Arg")]
    arg: Value,
    #[from_args(kw_only, default = Value::None, static_string = "ModuleKwarg")]
    module: Value,
    #[from_args(kw_only, default = Value::None)]
    owner: Value,
    #[from_args(kw_only, default = Value::Bool(true))]
    is_argument: Value,
    #[from_args(kw_only, default = Value::Bool(false))]
    is_class: Value,
}

/// Arguments of `ForwardRef.evaluate(*, globals=None, locals=None, type_params=None, owner=None, format=Format.VALUE)`.
#[derive(FromArgs)]
#[from_args(name = "ForwardRef.evaluate", style = def)]
struct EvaluateArgs {
    #[from_args(kw_only, default = Value::None)]
    globals: Value,
    #[from_args(kw_only, default = Value::None)]
    locals: Value,
    /// Accepted and ignored: Monty has no type parameters.
    #[from_args(kw_only, default = Value::None)]
    type_params: Value,
    #[from_args(kw_only, default = Value::None)]
    owner: Value,
    #[from_args(kw_only, default = Value::Int(1))]
    format: Value,
}

impl ForwardRef {
    /// `ForwardRef(arg, ...)`: `arg` must be a `str`; the keywords are stored as given.
    pub(crate) fn init(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
        let ForwardRefArgs {
            arg,
            module,
            owner,
            is_argument,
            is_class,
        } = ForwardRefArgs::from_args(args, vm)?;
        defer_drop!(is_argument, vm);
        defer_drop!(is_class, vm);
        defer_drop!(arg, vm);
        defer_drop!(module, vm);
        defer_drop!(owner, vm);
        let Some(text) = arg.as_str(vm) else {
            let repr = arg.py_repr(vm)?;
            defer_drop!(repr, vm);
            return Err(ExcType::type_error(format!(
                "Forward reference must be a string -- got {}",
                repr.as_str(vm).unwrap_or_default()
            )));
        };
        // Text from the constructor has no known shape, so it is always parenthesized
        // when composed, unless it is a bare name.
        let precedence = if is_identifier(text) {
            Precedence::Atom
        } else {
            Precedence::Tuple
        };
        let forward_ref = Self {
            arg: arg.clone_with_heap(vm),
            module: module.clone_with_heap(vm),
            owner: owner.clone_with_heap(vm),
            is_argument: is_argument.py_bool(vm)?,
            is_class: is_class.py_bool(vm)?,
            precedence,
            globals: None,
        };
        Ok(Value::Ref(
            vm.heap.allocate(HeapData::ForwardRef(Box::new(forward_ref))),
        ))
    }

    /// The stringifier a `FORWARDREF` evaluation against `globals` loads for the
    /// unresolved `name`. Takes ownership of `name` (a `str`), `owner` and a `globals` dict.
    pub(crate) fn stringifier(name: Value, owner: Value, is_class: bool, globals: FrameGlobals) -> Self {
        Self {
            arg: name,
            module: Value::None,
            owner,
            is_argument: false,
            is_class,
            precedence: Precedence::Atom,
            globals: Some(globals),
        }
    }

    /// The globals dict this reference owns, if any.
    fn globals_dict(&self) -> Option<HeapId> {
        match self.globals {
            Some(FrameGlobals::Dict(id)) => Some(id),
            Some(FrameGlobals::Slots) | None => None,
        }
    }

    /// Invokes `on_child` for each heap id this reference owns (GC trace hook).
    pub(crate) fn for_each_child_id(&self, mut on_child: impl FnMut(HeapId)) {
        for value in [&self.arg, &self.module, &self.owner] {
            if let Value::Ref(id) = value {
                on_child(*id);
            }
        }
        if let Some(id) = self.globals_dict() {
            on_child(id);
        }
    }
}

/// Releases a reference abandoned before it reaches the heap; a heap-stored one
/// is freed through [`HeapItem::py_dec_ref_ids`].
impl<C: ContainsHeap> DropWithContext<C> for ForwardRef {
    fn drop_with(self, ctx: &mut C) {
        if let Some(id) = self.globals_dict() {
            ctx.heap_mut().dec_ref(id);
        }
        self.arg.drop_with(ctx);
        self.module.drop_with(ctx);
        self.owner.drop_with(ctx);
    }
}

impl HeapItem for ForwardRef {
    fn py_dec_ref_ids(&mut self, stack: &mut Vec<HeapId>) {
        self.arg.py_dec_ref_ids(stack);
        self.module.py_dec_ref_ids(stack);
        self.owner.py_dec_ref_ids(stack);
        stack.extend(self.globals_dict());
    }
}

/// Generates a pair of binary-operator hooks that compose text while a
/// `FORWARDREF` evaluation runs and decline (`NotImplemented`) otherwise.
macro_rules! composing_binop {
    ($method:ident, $reflected:ident, $op:literal, $precedence:expr) => {
        fn $method(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
            self.compose_binop(other, $op, $precedence, false, vm)
        }

        fn $reflected(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
            self.compose_binop(other, $op, $precedence, true, vm)
        }
    };
}

/// Generates a unary-operator hook that composes text while a `FORWARDREF`
/// evaluation runs and declines otherwise.
macro_rules! composing_unary {
    ($method:ident, $op:literal) => {
        fn $method(&self, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
            if vm.in_forward_ref_evaluation() {
                let mut text = String::from($op);
                self.write_operand(Precedence::Factor, vm, &mut text);
                Ok(Some(self.compose(text, Precedence::Factor, vm)))
            } else {
                Ok(None)
            }
        }
    };
}

impl<'h> HeapObjectRead<'h, ForwardRef> {
    /// `__forward_arg__` as a Rust string.
    fn text(&self, vm: &VM<'h>) -> String {
        self.get(vm.heap).arg.as_str(vm).unwrap_or_default().to_owned()
    }

    /// Appends this reference's text, parenthesized when `demanded` binds tighter.
    fn write_operand(&self, demanded: Precedence, vm: &VM<'h>, out: &mut String) {
        let text = self.text(vm);
        if demanded > self.get(vm.heap).precedence {
            out.push('(');
            out.push_str(&text);
            out.push(')');
        } else {
            out.push_str(&text);
        }
    }

    /// A new reference holding `text`, with this one's owner, class flag and globals.
    fn compose(&self, text: String, precedence: Precedence, vm: &mut VM<'h>) -> Value {
        let this = self.get(vm.heap);
        if let Some(id) = this.globals_dict() {
            vm.heap.inc_ref(id);
        }
        let composed = ForwardRef {
            arg: allocate_string(text, vm.heap),
            module: Value::None,
            owner: this.owner.clone_with_heap(vm.heap),
            is_argument: false,
            is_class: this.is_class,
            precedence,
            globals: this.globals,
        };
        Value::Ref(vm.heap.allocate(HeapData::ForwardRef(Box::new(composed))))
    }

    /// `self op other` (or `other op self` when `reflected`) as composed text,
    /// following `ast.unparse`'s parenthesization; `None` outside a `FORWARDREF` evaluation.
    fn compose_binop(
        &self,
        other: &Value,
        op: &str,
        precedence: Precedence,
        reflected: bool,
        vm: &mut VM<'h>,
    ) -> RunResult<Option<Value>> {
        if !vm.in_forward_ref_evaluation() {
            return Ok(None);
        }
        // `**` is right-associative, so its left operand is the one that needs parentheses.
        let (left, right) = if precedence == Precedence::Power {
            (precedence.next(), precedence)
        } else {
            (precedence, precedence.next())
        };
        let mut text = String::new();
        if reflected {
            render_operand(other, left, vm, &mut text)?;
            write!(text, " {op} ")?;
            self.write_operand(right, vm, &mut text);
        } else {
            self.write_operand(left, vm, &mut text);
            write!(text, " {op} ")?;
            render_operand(other, right, vm, &mut text)?;
        }
        Ok(Some(self.compose(text, precedence, vm)))
    }

    /// `ForwardRef.evaluate(...)`: resolves the namespaces, then hands the text to the
    /// frozen evaluator, which applies the `VALUE` / `FORWARDREF` failure rules.
    fn evaluate(&self, args: ArgValues, vm: &mut VM<'h>) -> RunResult<CallResult> {
        let EvaluateArgs {
            globals,
            locals,
            type_params,
            owner,
            format,
        } = EvaluateArgs::from_args(args, vm)?;
        defer_drop!(globals, vm);
        defer_drop!(locals, vm);
        defer_drop!(type_params, vm);
        defer_drop!(owner, vm);
        defer_drop!(format, vm);
        let (arg, own_owner, is_class) = {
            let this = self.get(vm.heap);
            (
                this.arg.clone_with_heap(vm.heap),
                this.owner.clone_with_heap(vm.heap),
                this.is_class,
            )
        };
        defer_drop!(arg, vm);
        defer_drop!(own_owner, vm);
        let forwardref = match format_code(format) {
            Some(4) => return Ok(CallResult::Value(arg.clone_with_heap(vm))),
            Some(1) => false,
            Some(3) => true,
            _ => {
                let repr = format.py_repr(vm)?;
                defer_drop!(repr, vm);
                return Err(SimpleException::new_msg(
                    ExcType::NotImplementedError,
                    repr.as_str(vm).unwrap_or_default(),
                )
                .into());
            }
        };
        let owner = if matches!(owner, Value::None) { own_owner } else { owner };
        let globals = match globals {
            Value::None => typing::owner_globals(owner, vm),
            dict => dict.clone_with_heap(vm),
        };
        defer_drop!(globals, vm);
        let locals = match locals {
            Value::None => typing::owner_locals(owner, vm)?,
            dict => dict.clone_with_heap(vm),
        };
        defer_drop!(locals, vm);
        vm.call_frozen_exact(
            FrozenFunction::ForwardRefEvaluate,
            [
                self.clone_value(vm.heap),
                arg.clone_with_heap(vm),
                globals.clone_with_heap(vm),
                locals.clone_with_heap(vm),
                owner.clone_with_heap(vm),
                Value::Bool(is_class),
                Value::Bool(forwardref),
                Value::ModuleFunction(ModuleFunctions::Frozen(FrozenFunctions::EvalHint)),
                Value::Builtin(Builtins::Type(BuiltinType::ForwardRef)),
                Value::Builtin(Builtins::Function(BuiltinsFunctions::Isinstance)),
                Value::Builtin(Builtins::ExcType(ExcType::Exception)),
                Value::Builtin(Builtins::ExcType(ExcType::SyntaxError)),
            ],
        )
    }
}

impl<'h> PyTrait<'h> for HeapObjectRead<'h, ForwardRef> {
    fn py_type(&self, _: &VM<'h>) -> Type {
        Type::ForwardRef
    }

    fn py_len(&self, _: &VM<'h>) -> Option<usize> {
        None
    }

    /// Equal to another reference with the same text, module, class flag and
    /// owner, composed against the same globals (so a reference an evaluation
    /// made never equals a constructed one); `is_argument` is not compared, as in CPython.
    fn py_eq_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<bool>> {
        let Some(HeapReadOutput::ForwardRef(other)) = other.read_heap(vm) else {
            return Ok(None);
        };
        let (mine, theirs) = {
            let (this, that) = (self.get(vm.heap), other.get(vm.heap));
            if this.is_class != that.is_class || this.globals != that.globals {
                return Ok(Some(false));
            }
            (
                [
                    this.arg.clone_with_heap(vm.heap),
                    this.module.clone_with_heap(vm.heap),
                    this.owner.clone_with_heap(vm.heap),
                ],
                [
                    that.arg.clone_with_heap(vm.heap),
                    that.module.clone_with_heap(vm.heap),
                    that.owner.clone_with_heap(vm.heap),
                ],
            )
        };
        defer_drop!(mine, vm);
        defer_drop!(theirs, vm);
        for (a, b) in mine.iter().zip(theirs) {
            if !a.py_eq(b, vm)? {
                return Ok(Some(false));
            }
        }
        Ok(Some(true))
    }

    /// Hashes what [`Self::py_eq_impl`] compares; an unhashable owner makes the
    /// reference unhashable, as CPython's `hash((..., owner))` raises.
    fn py_hash(&self, vm: &mut VM<'h>) -> RunResult<Option<HashValue>> {
        let (parts, is_class, globals) = {
            let this = self.get(vm.heap);
            (
                [
                    this.arg.clone_with_heap(vm.heap),
                    this.module.clone_with_heap(vm.heap),
                    this.owner.clone_with_heap(vm.heap),
                ],
                this.is_class,
                this.globals,
            )
        };
        defer_drop!(parts, vm);
        let mut hasher = AHasher::default();
        is_class.hash(&mut hasher);
        // A dict's identity is its heap id, which the hash must not persist across dumps
        // under another meaning; dict and set entries rehash a reference by these parts.
        match globals {
            None => 0u8.hash(&mut hasher),
            Some(FrameGlobals::Slots) => 1u8.hash(&mut hasher),
            Some(FrameGlobals::Dict(id)) => identity_hash(id).hash(&mut hasher),
        }
        for part in parts {
            let Some(hash) = part.py_hash(vm)? else {
                return Ok(None);
            };
            hash.hash(&mut hasher);
        }
        Ok(Some(HashValue::new(hasher.finish())))
    }

    /// `ForwardRef('Later')`, then `module=`, `is_class=True` and `owner=` when set.
    fn py_repr_fmt(&self, f: &mut impl Write, vm: &mut VM<'h>, heap_ids: &mut LazyHeapSet) -> RunResult<()> {
        let (text, module, owner, is_class) = {
            let this = self.get(vm.heap);
            (
                self.text(vm),
                this.module.clone_with_heap(vm.heap),
                this.owner.clone_with_heap(vm.heap),
                this.is_class,
            )
        };
        defer_drop!(module, vm);
        defer_drop!(owner, vm);
        write!(f, "ForwardRef({}", StringRepr(&text))?;
        if !matches!(module, Value::None) {
            f.write_str(", module=")?;
            module.py_repr_fmt(f, vm, heap_ids)?;
        }
        if is_class {
            f.write_str(", is_class=True")?;
        }
        if !matches!(owner, Value::None) {
            f.write_str(", owner=")?;
            owner.py_repr_fmt(f, vm, heap_ids)?;
        }
        Ok(f.write_char(')')?)
    }

    /// The `__forward_*__` introspection attributes; any other name composes
    /// `Later.attr` during a `FORWARDREF` evaluation and is an `AttributeError` otherwise.
    fn py_getattr(&self, attr: &EitherStr, vm: &mut VM<'h>) -> RunResult<Option<CallResult>> {
        let this = self.get(vm.heap);
        let value = match attr.static_string(vm.interns) {
            Some(StaticStrings::DunderForwardArg | StaticStrings::DunderResolvedStr) => {
                this.arg.clone_with_heap(vm.heap)
            }
            Some(StaticStrings::DunderForwardModule) => this.module.clone_with_heap(vm.heap),
            Some(StaticStrings::DunderOwner) => this.owner.clone_with_heap(vm.heap),
            Some(StaticStrings::DunderForwardIsArgument) => Value::Bool(this.is_argument),
            Some(StaticStrings::DunderForwardIsClass) => Value::Bool(this.is_class),
            _ if vm.in_forward_ref_evaluation() => {
                let mut text = String::new();
                self.write_operand(Precedence::Atom, vm, &mut text);
                text.push('.');
                text.push_str(attr.as_str(vm.interns));
                self.compose(text, Precedence::Atom, vm)
            }
            // Named as CPython does, not by the qualified type name.
            _ => return Err(ExcType::attribute_error("ForwardRef", attr.as_str(vm.interns))),
        };
        Ok(Some(CallResult::Value(value)))
    }

    /// `evaluate(...)`; any other method composes `Later.method(...)` during a
    /// `FORWARDREF` evaluation.
    fn py_call_attr(&mut self, vm: &mut VM<'h>, attr: &EitherStr, args: ArgValues) -> RunResult<CallResult> {
        if attr.static_string(vm.interns) == Some(StaticStrings::Evaluate) {
            self.evaluate(args, vm)
        } else if vm.in_forward_ref_evaluation() {
            let mut text = String::new();
            self.write_operand(Precedence::Atom, vm, &mut text);
            text.push('.');
            text.push_str(attr.as_str(vm.interns));
            write_call(&mut text, args, vm)?;
            Ok(CallResult::Value(self.compose(text, Precedence::Atom, vm)))
        } else {
            args.drop_with(vm);
            Err(ExcType::attribute_error("ForwardRef", attr.as_str(vm.interns)))
        }
    }

    /// `Later(1, x=Foo)` composes during a `FORWARDREF` evaluation; a plain
    /// reference is not callable.
    fn py_call(&mut self, args: ArgValues, vm: &mut VM<'h>) -> RunResult<CallResult> {
        if vm.in_forward_ref_evaluation() {
            let mut text = String::new();
            self.write_operand(Precedence::Atom, vm, &mut text);
            write_call(&mut text, args, vm)?;
            Ok(CallResult::Value(self.compose(text, Precedence::Atom, vm)))
        } else {
            args.drop_with(vm);
            Err(ExcType::type_error_not_callable_object("ForwardRef"))
        }
    }

    /// `Later[int]`, `Later[int, str]` and `Later[1:2]` compose during a
    /// `FORWARDREF` evaluation; a plain reference is not subscriptable.
    fn py_getitem(&self, key: &Value, vm: &mut VM<'h>) -> RunResult<Value> {
        if !vm.in_forward_ref_evaluation() {
            return Err(ExcType::type_error_not_sub("ForwardRef"));
        }
        let mut text = String::new();
        self.write_operand(Precedence::Atom, vm, &mut text);
        text.push('[');
        match key {
            // A tuple key lists its items bare, as `ast.unparse` writes a subscript.
            Value::Ref(id) if let HeapData::Tuple(tuple) = vm.heap.get(*id) => {
                let items = clone_items(tuple.as_slice(), vm)?;
                defer_drop!(items, vm);
                render_items(items, vm, &mut text)?;
            }
            Value::Ref(id) if let HeapData::Slice(slice) = vm.heap.get(*id) => {
                let (start, stop, step) = (slice.start, slice.stop, slice.step);
                for (index, bound) in [start, stop].into_iter().enumerate() {
                    if index > 0 {
                        text.push(':');
                    }
                    if let Some(bound) = bound {
                        write!(text, "{bound}")?;
                    }
                }
                if let Some(step) = step {
                    write!(text, ":{step}")?;
                }
            }
            _ => render_operand(key, Precedence::Test, vm, &mut text)?,
        }
        text.push(']');
        Ok(self.compose(text, Precedence::Atom, vm))
    }

    /// `|` composes during a `FORWARDREF` evaluation; otherwise, as CPython's
    /// `ForwardRef.__or__`, it builds a union with any right operand.
    fn py_or_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        if let Some(composed) = self.compose_binop(other, "|", Precedence::BitOr, false, vm)? {
            Ok(Some(composed))
        } else {
            let members = vec![self.clone_value(vm.heap), other.clone_with_heap(vm)];
            Union::from_members(members, vm).map(Some)
        }
    }

    fn py_ror_impl(&self, other: &Value, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        if let Some(composed) = self.compose_binop(other, "|", Precedence::BitOr, true, vm)? {
            Ok(Some(composed))
        } else {
            let members = vec![other.clone_with_heap(vm), self.clone_value(vm.heap)];
            Union::from_members(members, vm).map(Some)
        }
    }

    composing_binop!(py_add_impl, py_radd_impl, "+", Precedence::Arith);
    composing_binop!(py_sub_impl, py_rsub_impl, "-", Precedence::Arith);
    composing_binop!(py_mul_impl, py_rmul_impl, "*", Precedence::Term);
    composing_binop!(py_matmul_impl, py_rmatmul_impl, "@", Precedence::Term);
    composing_binop!(py_truediv_impl, py_rtruediv_impl, "/", Precedence::Term);
    composing_binop!(py_floordiv_impl, py_rfloordiv_impl, "//", Precedence::Term);
    composing_binop!(py_mod_impl, py_rmod_impl, "%", Precedence::Term);
    composing_binop!(py_lshift_impl, py_rlshift_impl, "<<", Precedence::Shift);
    composing_binop!(py_rshift_impl, py_rrshift_impl, ">>", Precedence::Shift);
    composing_binop!(py_and_impl, py_rand_impl, "&", Precedence::BitAnd);
    composing_binop!(py_xor_impl, py_rxor_impl, "^", Precedence::BitXor);

    fn py_pow_impl(&self, other: &Value, modulus: Option<&Value>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        if modulus.is_some() {
            Ok(None)
        } else {
            self.compose_binop(other, "**", Precedence::Power, false, vm)
        }
    }

    fn py_rpow_impl(&self, other: &Value, modulus: Option<&Value>, vm: &mut VM<'h>) -> RunResult<Option<Value>> {
        if modulus.is_some() {
            Ok(None)
        } else {
            self.compose_binop(other, "**", Precedence::Power, true, vm)
        }
    }

    composing_unary!(py_neg_impl, "-");
    composing_unary!(py_pos_impl, "+");
    composing_unary!(py_invert_impl, "~");
}

/// Appends `(a, b, k=v)` for a composed call, consuming `args`.
fn write_call(text: &mut String, args: ArgValues, vm: &mut VM<'_>) -> RunResult<()> {
    let (positional, keywords) = args.into_parts();
    let positional: Vec<Value> = positional.collect();
    defer_drop!(positional, vm);
    let keywords: Vec<(Value, Value)> = keywords.into_iter().collect();
    defer_drop!(keywords, vm);
    text.push('(');
    render_items(positional, vm, text)?;
    for (index, (name, value)) in keywords.iter().enumerate() {
        if index > 0 || !positional.is_empty() {
            text.push_str(", ");
        }
        text.push_str(name.as_str(vm).unwrap_or_default());
        text.push('=');
        render_operand(value, Precedence::Test, vm, text)?;
    }
    text.push(')');
    Ok(())
}

/// Appends `items` comma-separated, each rendered as an argument.
fn render_items(items: &[Value], vm: &mut VM<'_>, text: &mut String) -> RunResult<()> {
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            text.push_str(", ");
        }
        render_operand(item, Precedence::Test, vm, text)?;
    }
    Ok(())
}

/// Appends `value` as CPython's `_Stringifier` would unparse it: another
/// reference by its text (parenthesized when `demanded` binds tighter), a tuple,
/// list, set or dict item by item, a type by its name and anything else by its repr.
fn render_operand(value: &Value, demanded: Precedence, vm: &mut VM<'_>, text: &mut String) -> RunResult<()> {
    let mut guard = vm.recursion_guard()?;
    let vm = &mut *guard;
    let container = match value {
        Value::Ref(id) => match vm.heap.get(*id) {
            HeapData::ForwardRef(forward_ref) => {
                let inner = forward_ref.arg.as_str_heap(vm.heap, vm.interns).unwrap_or_default();
                if demanded > forward_ref.precedence {
                    write!(text, "({inner})")?;
                } else {
                    text.push_str(inner);
                }
                return Ok(());
            }
            HeapData::Tuple(tuple) => Container::Tuple(clone_items(tuple.as_slice(), vm)?),
            HeapData::List(list) => Container::List(clone_items(list.as_slice(), vm)?),
            HeapData::Set(set) => {
                let items: Vec<&Value> = set.iter().collect();
                vm.heap
                    .tracker
                    .check_allocation(items.len().saturating_mul(VALUE_SIZE))?;
                Container::Set(items.into_iter().map(|item| item.clone_with_heap(vm.heap)).collect())
            }
            HeapData::Dict(dict) => {
                let pairs: Vec<(&Value, &Value)> = dict.iter().collect();
                vm.heap
                    .tracker
                    .check_allocation(pairs.len().saturating_mul(2 * VALUE_SIZE))?;
                Container::Dict(
                    pairs
                        .into_iter()
                        .map(|(k, v)| (k.clone_with_heap(vm.heap), v.clone_with_heap(vm.heap)))
                        .collect(),
                )
            }
            _ => return repr_type_arg(value, text, vm, &mut LazyHeapSet::default()),
        },
        _ => return repr_type_arg(value, text, vm, &mut LazyHeapSet::default()),
    };
    defer_drop!(container, vm);
    match container {
        Container::Tuple(items) => {
            text.push('(');
            render_items(items, vm, text)?;
            if items.len() == 1 {
                text.push(',');
            }
            text.push(')');
        }
        Container::List(items) => {
            text.push('[');
            render_items(items, vm, text)?;
            text.push(']');
        }
        Container::Set(items) if items.is_empty() => text.push_str("set()"),
        Container::Set(items) => {
            text.push('{');
            render_items(items, vm, text)?;
            text.push('}');
        }
        Container::Dict(pairs) => {
            text.push('{');
            for (index, (key, value)) in pairs.iter().enumerate() {
                if index > 0 {
                    text.push_str(", ");
                }
                render_operand(key, Precedence::Test, vm, text)?;
                text.push_str(": ");
                render_operand(value, Precedence::Test, vm, text)?;
            }
            text.push('}');
        }
    }
    Ok(())
}

/// Owned clones of a container's items, taken before rendering them so the
/// heap borrow ends first.
enum Container {
    Tuple(Vec<Value>),
    List(Vec<Value>),
    Set(Vec<Value>),
    Dict(Vec<(Value, Value)>),
}

impl<C: ContainsHeap> DropWithContext<C> for Container {
    fn drop_with(self, ctx: &mut C) {
        match self {
            Self::Tuple(items) | Self::List(items) | Self::Set(items) => items.drop_with(ctx),
            Self::Dict(pairs) => pairs.drop_with(ctx),
        }
    }
}

/// Owned clones of `items`, preflighted against the memory limit.
fn clone_items(items: &[Value], vm: &VM<'_>) -> RunResult<Vec<Value>> {
    vm.heap
        .tracker
        .check_allocation(items.len().saturating_mul(VALUE_SIZE))?;
    Ok(items.iter().map(|item| item.clone_with_heap(vm.heap)).collect())
}

/// The `Format` member `value` names, `None` for anything else. CPython compares
/// with `==`, so a bool or a whole float (`4.0`) names the member too.
pub(crate) fn format_code(value: &Value) -> Option<i64> {
    match value {
        Value::Int(code) => Some(*code),
        Value::Bool(flag) => Some(i64::from(*flag)),
        Value::Float(code) => whole_float_code(*code),
        _ => None,
    }
}

/// A small whole float as its int; `None` for a fraction or anything beyond the members.
#[expect(clippy::cast_possible_truncation, reason = "a whole float below 16 converts exactly")]
fn whole_float_code(code: f64) -> Option<i64> {
    (code.fract() == 0.0 && code.abs() < 16.0).then_some(code as i64)
}

/// Whether `text` is a plain Python identifier, which `ForwardRef.evaluate()`
/// looks up as a name rather than compiling.
fn is_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(|first| first == '_' || first.is_alphabetic())
        && chars.all(|c| c == '_' || c.is_alphanumeric())
}
