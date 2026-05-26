# `os` module

The sandbox deliberately exposes almost nothing from `os`. Filesystem
work goes via [pathlib](pathlib.md) and [open](open.md), which route
through the host's mount table.

## Implemented

- `os.getenv(key, default=None)` — yields to the host; the host decides
  which environment variables are visible (typically a curated subset, not
  the full host environment).
- `os.environ` — property that yields to the host and returns a `dict` of
  the same curated environment. It is a plain dict, not an `os._Environ`
  object: mutating it does **not** propagate back to the host.

## Not implemented

Everything else, including but not limited to: `os.path.*` (use
`pathlib.Path` instead), `os.getcwd`, `os.chdir`, `os.listdir`,
`os.walk`, `os.scandir`, `os.mkdir`, `os.makedirs`, `os.remove`,
`os.unlink`, `os.rmdir`, `os.rename`, `os.replace`, `os.stat`, `os.lstat`,
`os.access`, `os.symlink`, `os.readlink`, `os.chmod`, `os.chown`,
`os.umask`, `os.system`, `os.popen`, `os.fork`, `os.exec*`, `os.spawn*`,
`os.kill`, `os.pipe`, `os.read`, `os.write`, `os.open`, `os.close`,
`os.dup`, `os.fsync`, `os.urandom`, `os.cpu_count`, `os.getpid`,
`os.getuid`, `os.getgid`, `os.uname`.

`subprocess`, `signal`, `socket`, `threading`, `multiprocessing` are not
importable either (see [modules.md](modules.md)).
