fn main() {
    // Embed libpython: add the rpath link args so the binary finds the linked
    // Python shared library at runtime (mirrors `monty-datatest`, the other
    // crate that embeds CPython via pyo3's `auto-initialize`).
    pyo3_build_config::add_libpython_rpath_link_args();
}
