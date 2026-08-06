#[derive(Debug, Clone, Copy, Default, strum::Display)]
#[strum(serialize_all = "lowercase")]
pub enum TypeCheckingFormat {
    #[default]
    Full,
    Concise,
    Azure,
    Json,
    JsonLines,
    Rdjson,
    Pylint,
    Gitlab,
    Github,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TypeCheckingConfig {
    pub format: TypeCheckingFormat,
    pub color: bool,
}
