use monty::MontyRun;
use monty_types::{CompileOptions, MontyObject};

/// Linux values remain fixed when Monty runs on macOS or Windows.
#[test]
fn linux_errno_values_and_aliases() {
    let code = r"
import errno
assert errno.EAGAIN == errno.EWOULDBLOCK == 11
assert errno.EDEADLK == errno.EDEADLOCK == 35
assert errno.EOPNOTSUPP == errno.ENOTSUP == 95
assert errno.ENOSYS == 38
assert errno.ENOTEMPTY == 39
assert errno.ELOOP == 40
assert errno.ECONNREFUSED == 111
assert errno.EOWNERDEAD == 130
assert errno.ENOTRECOVERABLE == 131
assert errno.ERFKILL == 132
assert errno.EHWPOISON == 133
assert len(errno.errorcode) == 131
assert errno.errorcode[11] == 'EAGAIN'
assert errno.errorcode[35] == 'EDEADLOCK'
assert errno.errorcode[95] == 'ENOTSUP'
assert 41 not in errno.errorcode
assert 58 not in errno.errorcode
assert not hasattr(errno, 'WSAEWOULDBLOCK')
assert not hasattr(errno, 'EFTYPE')
True
";
    let output = MontyRun::new(code.to_owned(), "errno.py", vec![], CompileOptions::default())
        .unwrap()
        .run_no_limits(vec![])
        .unwrap();
    assert_eq!(output, MontyObject::bool(true));
}
