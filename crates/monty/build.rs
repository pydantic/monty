fn main() {
    // This ensures that tests can find the libpython shared library at runtime, even if it's not on
    // the system library path. This makes running tests much easier on e.g. Linux with a uv venv.
    //
    // This is technically a bit wasteful because the main `lib` doesn't need this, just tests, but it
    // won't affect downstream executables other than requiring them to have a valid Python in their system.
    //
    // If that becomes a big problem, we can rethink.
    pyo3_build_config::add_libpython_rpath_link_args();
}
