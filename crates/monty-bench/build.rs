fn main() {
    // This ensures that benchmarks can find the libpython shared library at runtime, even if it's
    // not on the system library path. This makes running benchmarks much easier on e.g. Linux with
    // a uv venv.
    pyo3_build_config::add_libpython_rpath_link_args();
}
