use pyo3::{Bound, Py, PyAny, PyResult, Python, sync::PyOnceLock};

pub fn get_re_pattern_error(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static RE_PATTERN_ERROR: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

    RE_PATTERN_ERROR.import(py, "re", "PatternError")
}
