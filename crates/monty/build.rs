fn main() {
    // This ensures that the tests and benchmarks link to exact path to libpython, which avoids need
    // to manually set LD_LIBRARY_PATH when running them.
    pyo3_build_config::add_libpython_rpath_link_args();
}
