//! The `ConfigDict` settings Monty honours, read from the dict `ConfigDict(...)`
//! returns: `TypeAdapter(type, config=...)` for a plain type, or a dataclass's
//! `__pydantic_config__` attribute.
//!
//! The keys `limitations/pydantic.md` lists are read; pydantic's other keys
//! raise `NotImplementedError` naming the key, and keys pydantic does not know
//! are ignored, as pydantic ignores them.

use crate::{
    bytecode::VM,
    exception_private::{ExcType, ExcTypeExt, RunError, RunResult, SimpleException},
    modules::pydantic::errors::dict_pairs,
    value::Value,
};

/// What a config does to validation and errors.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[expect(clippy::struct_excessive_bools, reason = "one bool per config flag")]
pub(crate) struct Config {
    /// `strict`: no lax coercions anywhere under the config.
    pub(crate) strict: bool,
    /// `extra='forbid'`: a dataclass input key that names no field is an error;
    /// `ignore` and `allow` both drop it, as pydantic does for a dataclass.
    pub(crate) extra_forbid: bool,
    /// `populate_by_name`: an aliased dataclass field is also read by its name.
    pub(crate) populate_by_name: bool,
    pub(crate) str_strip_whitespace: bool,
    pub(crate) str_to_lower: bool,
    pub(crate) str_to_upper: bool,
    pub(crate) str_min_length: Option<usize>,
    pub(crate) str_max_length: Option<usize>,
    pub(crate) coerce_numbers_to_str: bool,
    /// `title`: the `ValidationError` title of a `TypeAdapter` built with the config.
    pub(crate) title: Option<String>,
}

/// pydantic's `ConfigDict` keys Monty does not model, refused when present.
const UNSUPPORTED_KEYS: &[&str] = &[
    "model_title_generator",
    "field_title_generator",
    "frozen",
    "use_enum_values",
    "validate_assignment",
    "arbitrary_types_allowed",
    "from_attributes",
    "loc_by_alias",
    "alias_generator",
    "ignored_types",
    "allow_inf_nan",
    "json_schema_extra",
    "json_encoders",
    "revalidate_instances",
    "ser_json_timedelta",
    "ser_json_temporal",
    "val_temporal_unit",
    "ser_json_bytes",
    "val_json_bytes",
    "ser_json_inf_nan",
    "validate_default",
    "validate_return",
    "protected_namespaces",
    "hide_input_in_errors",
    "defer_build",
    "plugin_settings",
    "schema_generator",
    "json_schema_serialization_defaults_required",
    "json_schema_mode_override",
    "regex_engine",
    "validation_error_cause",
    "use_attribute_docstrings",
    "cache_strings",
    "validate_by_alias",
    "validate_by_name",
    "serialize_by_alias",
    "url_preserve_empty_path",
    "polymorphic_serialization",
];

impl Config {
    /// Reads a config from the dict `value`; `None` is the empty config.
    pub(crate) fn parse(value: &Value, vm: &mut VM<'_>) -> RunResult<Self> {
        let mut config = Self::default();
        if matches!(value, Value::None) {
            return Ok(config);
        }
        let Some(pairs) = dict_pairs(value, vm) else {
            return Err(ExcType::type_error(format!(
                "config must be a dict, not {}",
                value.py_type_name(vm)
            )));
        };
        let mut result = Ok(());
        for (key, setting) in &pairs {
            if result.is_ok() {
                result = match key.as_str(vm) {
                    Some(name) => config.apply(name, setting, vm),
                    None => Err(ExcType::type_error("config keys must be str")),
                };
            }
        }
        for (key, setting) in pairs {
            key.drop_with(vm);
            setting.drop_with(vm);
        }
        result.map(|()| config)
    }

    /// Applies one setting, with pydantic's `ValueError` for a bad value.
    fn apply(&mut self, name: &str, setting: &Value, vm: &VM<'_>) -> RunResult<()> {
        let flag = || match setting {
            Value::None => Ok(false),
            Value::Bool(flag) => Ok(*flag),
            other => Err(setting_type_error(name, "bool", other, vm)),
        };
        let length = || match setting {
            Value::None => Ok(None),
            Value::Int(n) if *n >= 0 => Ok(Some(usize::try_from(*n).expect("non-negative i64 fits usize"))),
            Value::Int(_) => Err(ExcType::value_error(format!("config '{name}' must be non-negative"))),
            other => Err(setting_type_error(name, "int", other, vm)),
        };
        match name {
            "strict" => self.strict = flag()?,
            "populate_by_name" => self.populate_by_name = flag()?,
            "str_strip_whitespace" => self.str_strip_whitespace = flag()?,
            "str_to_lower" => self.str_to_lower = flag()?,
            "str_to_upper" => self.str_to_upper = flag()?,
            "coerce_numbers_to_str" => self.coerce_numbers_to_str = flag()?,
            "str_min_length" => self.str_min_length = length()?,
            "str_max_length" => self.str_max_length = length()?,
            "title" => {
                self.title = match setting {
                    Value::None => None,
                    other => match other.as_str(vm) {
                        Some(title) => Some(title.to_owned()),
                        None => return Err(setting_type_error(name, "str", other, vm)),
                    },
                };
            }
            "extra" => {
                // A dataclass has nowhere to keep extra keys, so pydantic drops them under `allow` too.
                self.extra_forbid = match setting.as_str(vm) {
                    Some("ignore" | "allow") => false,
                    Some("forbid") => true,
                    Some(other) => {
                        return Err(ExcType::value_error(format!(
                            "Input should be 'allow', 'forbid' or 'ignore', not {other:?}"
                        )));
                    }
                    None if matches!(setting, Value::None) => false,
                    None => return Err(setting_type_error(name, "str", setting, vm)),
                };
            }
            other if UNSUPPORTED_KEYS.contains(&other) => return Err(unsupported_key(other)),
            _ => {}
        }
        Ok(())
    }
}

/// `NotImplementedError` for a config key Monty does not model.
fn unsupported_key(key: &str) -> RunError {
    SimpleException::new_msg(
        ExcType::NotImplementedError,
        format!("ConfigDict key '{key}' is not supported"),
    )
    .into()
}

/// `TypeError` for a setting of the wrong type.
fn setting_type_error(key: &str, expected: &str, value: &Value, vm: &VM<'_>) -> RunError {
    ExcType::type_error(format!(
        "config '{key}' must be {expected}, not {}",
        value.py_type_name(vm)
    ))
}
