# stat is vendored from CPython's own source, with a patch; see crates/monty/stdlib/
import stat

# === File type tests ===
assert stat.S_ISDIR(0o040755) is True
assert stat.S_ISDIR(0o100644) is False
assert stat.S_ISREG(0o100644) is True
assert stat.S_ISLNK(0o120777) is True
assert stat.S_ISFIFO(0o010644) is True
assert stat.S_ISSOCK(0o140755) is True
assert stat.S_ISCHR(0o020600) is True
assert stat.S_ISBLK(0o060600) is True

# === Mode parts ===
assert stat.S_IMODE(0o100644) == 0o644
assert stat.S_IMODE(0o104755) == 0o4755
assert stat.S_IFMT(0o100644) == stat.S_IFREG
assert stat.S_IFMT(0o040755) == stat.S_IFDIR

# === filemode ===
assert stat.filemode(0o100644) == '-rw-r--r--'
assert stat.filemode(0o040755) == 'drwxr-xr-x'
assert stat.filemode(0o104755) == '-rwsr-xr-x'
assert stat.filemode(0o120777) == 'lrwxrwxrwx'
assert stat.filemode(0o041777) == 'drwxrwxrwt'
assert stat.filemode(0o020600) == 'crw-------'
assert stat.filemode(0o102644) == '-rw-r-Sr--'

# === Constants ===
assert (stat.ST_MODE, stat.ST_SIZE, stat.ST_MTIME) == (0, 6, 8)
assert (stat.S_IFDIR, stat.S_IFREG, stat.S_IFLNK) == (0o040000, 0o100000, 0o120000)
assert (stat.S_IRWXU, stat.S_IRUSR, stat.S_IWUSR, stat.S_IXUSR) == (0o700, 0o400, 0o200, 0o100)
assert (stat.S_ISUID, stat.S_ISGID, stat.S_ISVTX) == (0o4000, 0o2000, 0o1000)
assert stat.UF_HIDDEN == 0x00008000
assert stat.FILE_ATTRIBUTE_DIRECTORY == 16
