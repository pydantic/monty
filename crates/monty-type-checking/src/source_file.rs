/// Definition of a source file handed to a type checker.
pub struct SourceFile<'a> {
    /// source code
    pub source_code: &'a str,
    /// file path
    pub path: &'a str,
}

impl<'a> SourceFile<'a> {
    /// Create a new source file.
    #[must_use]
    pub fn new(source_code: &'a str, path: &'a str) -> Self {
        Self { source_code, path }
    }
}
