/// Convert this crate's Cargo version into a PEP 440 version, e.g.
/// `0.0.19-beta.2` -> `0.0.19b.2` (which PEP 440 normalizes to `0.0.19b2`).
///
/// Copied from `get_pydantic_core_version` in pydantic. Cargo uses `1.0-alpha1`
/// etc. while python uses `1.0.0a1`; this is not full compatibility, but it's
/// good enough for now. The dot after `alpha`/`beta` (e.g. `-alpha.1`) does not
/// need removing, hence why this works.
///
/// See <https://docs.rs/semver/1.0.9/semver/struct.Version.html#method.parse> for
/// the rust spec and <https://peps.python.org/pep-0440/> for the python spec.
///
/// Shared by `lib.rs` (for `pydantic_monty.__version__`) and `build.rs` (for the
/// exact `pydantic-monty-runtime` pin), which `include!`s this file — the two
/// must agree, since the pin has to match the version maturin builds the runtime
/// wheel under.
pub(crate) fn cargo_version_to_pep440(version: &str) -> String {
    version.replace("-alpha", "a").replace("-beta", "b")
}
