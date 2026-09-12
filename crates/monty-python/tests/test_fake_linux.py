"""Tests for FakeLinux — the deterministic fake Linux system.

FakeLinux subclasses OSAccess with a fully synthetic Linux tree, environ and
system-identity answers. Tests run Python code through Monty to verify the
illusion from the sandbox's point of view, and assert host-side state (recorded
`os.system` commands) directly.
"""

import datetime

import pytest
from conftest import RunMonty
from inline_snapshot import snapshot

from pydantic_monty import FakeLinux, MemoryFile, MontyRuntimeError, OSAccess


def test_uname(monty_run: RunMonty):
    """os.uname() returns the synthetic identity with attribute and index access."""
    fs = FakeLinux(hostname='prod-web-1', kernel='6.1.0-fake', machine='aarch64')
    result = monty_run(
        'import os\nu = os.uname()\n(u.sysname, u.nodename, u.release, u.version, u.machine, u[0], len(u))',
        os=fs,
    )
    assert result == snapshot(('Linux', 'prod-web-1', '6.1.0-fake', '#1 SMP PREEMPT_DYNAMIC', 'aarch64', 'Linux', 5))


def test_getcwd_cpu_count_getpid(monty_run: RunMonty):
    """The identity calls answer the configured synthetic values."""
    fs = FakeLinux(cwd='/home/monty/work', cpu_count=3, pid=7)
    result = monty_run('import os\n(os.getcwd(), os.cpu_count(), os.getpid())', os=fs)
    assert result == snapshot(('/home/monty/work', 3, 7))


def test_system_records_and_returns_zero(monty_run: RunMonty):
    """os.system records the command host-side and returns 0; nothing executes."""
    fs = FakeLinux()
    result = monty_run(
        "import os\nrc = os.system('apt-get install -y nothing')\nrc2 = os.system(b'uptime')\n(rc, rc2)",
        os=fs,
    )
    assert result == snapshot((0, 0))
    assert fs.commands == snapshot(['apt-get install -y nothing', 'uptime'])


def test_system_handler_override(monty_run: RunMonty):
    """A system_handler answers os.system instead of the default zero."""
    fs = FakeLinux(system_handler=lambda command: 42 if 'rm' in command else 1)
    result = monty_run("import os\n(os.system('rm -rf /'), os.system('ls'))", os=fs)
    assert result == snapshot((42, 1))
    assert fs.commands == snapshot(['rm -rf /', 'ls'])


def test_environ_defaults_and_overrides(monty_run: RunMonty):
    """Generated environ defaults apply; user environ wins."""
    fs = FakeLinux(user='ada', hostname='h1', environ={'EXTRA': 'yes', 'USER': 'override'})
    result = monty_run(
        'import os\n'
        "os.environ['PATH'], os.environ['HOME'], os.environ['USER'], os.environ['HOSTNAME'], "
        "os.environ['EXTRA'], os.environ.get('MISSING')",
        os=fs,
    )
    assert result == snapshot(
        (
            '/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin',
            '/home/ada',
            'override',
            'h1',
            'yes',
            None,
        )
    )


def test_etc_files(monty_run: RunMonty):
    """/etc contains the standard files with deterministic content."""
    fs = FakeLinux(hostname='web-1', distro='debian-12')
    result = monty_run(
        'from pathlib import Path\n'
        "Path('/etc/hostname').read_text(), "
        "sorted(Path('/etc').listdir()) if False else sorted(p.name for p in Path('/etc').iterdir())",
        os=fs,
    )
    assert result[0] == snapshot('web-1\n')
    assert result[1] == snapshot(['group', 'hostname', 'hosts', 'os-release', 'passwd', 'resolv.conf', 'timezone'])


def test_os_release_presets(monty_run: RunMonty):
    """Preset distros render real os-release content; custom dicts pass through."""
    ubuntu = FakeLinux(distro='ubuntu-24.04')
    result = monty_run("from pathlib import Path\nPath('/etc/os-release').read_text()", os=ubuntu)
    assert result == snapshot(
        'NAME="Ubuntu"\nVERSION_ID="24.04"\nID="ubuntu"\nPRETTY_NAME="Ubuntu 24.04.1 LTS"\n'
        'VERSION="24.04.1 LTS (Noble Numbat)"\nVERSION_CODENAME="noble"\nID_LIKE="debian"\n'
        'HOME_URL="https://www.ubuntu.com/"\n'
    )

    custom = FakeLinux(distro={'NAME': 'FakeOS', 'ID': 'fakeos'})
    result = monty_run("from pathlib import Path\nPath('/etc/os-release').read_text()", os=custom)
    assert result == snapshot('NAME="FakeOS"\nID="fakeos"\n')


def test_proc_files(monty_run: RunMonty):
    """/proc exposes cpuinfo, meminfo, version, uptime, loadavg and self."""
    fs = FakeLinux(cpu_count=4, memory_total=2 * 1024**3, pid=99, kernel='6.6.0-fake')
    result = monty_run(
        'from pathlib import Path\n'
        "cpu = Path('/proc/cpuinfo').read_text()\n"
        "mem = Path('/proc/meminfo').read_text()\n"
        "ver = Path('/proc/version').read_text()\n"
        "up = Path('/proc/uptime').read_text()\n"
        "load = Path('/proc/loadavg').read_text()\n"
        "comm = Path('/proc/self/comm').read_text()\n"
        "(cpu.count('processor'), 'Monty Virtual CPU' in cpu, 'MemTotal:' in mem, '2097152' in mem, "
        "'6.6.0-fake' in ver, up, '99' in load, comm)",
        os=fs,
    )
    assert result == snapshot((4, True, True, True, True, '86400.00 345600.00\n', True, 'python3\n'))


def test_dev_null(monty_run: RunMonty):
    """/dev/null discards writes and reads back empty."""
    fs = FakeLinux()
    result = monty_run(
        'from pathlib import Path\n'
        "Path('/dev/null').write_text('discard me')\n"
        "Path('/dev/null').write_bytes(b'binary too')\n"
        "Path('/dev/null').read_text()",
        os=fs,
    )
    assert result == snapshot('')


def test_usr_bin_executables(monty_run: RunMonty):
    """/usr/bin advertises the usual tools as executable regular files."""
    fs = FakeLinux()
    result = monty_run(
        'import os\nfrom pathlib import Path\n'
        "names = sorted(p.name for p in Path('/usr/bin').iterdir())\n"
        "modes = [os.stat(Path('/usr/bin/sh')).st_mode, os.stat(Path('/usr/bin/sh')).st_mode & 0o777]\n"
        '(names, modes)',
        os=fs,
    )
    assert result[0] == snapshot(['awk', 'bash', 'cat', 'env', 'grep', 'ls', 'python3', 'sed', 'sh', 'uname'])
    assert result[1][1] == snapshot(0o755)


def test_extra_files_layered(monty_run: RunMonty):
    """User files are added on top of the generated tree."""
    fs = FakeLinux(files=[MemoryFile('/work/data.csv', content='a,b\n1,2\n')])
    result = monty_run("from pathlib import Path\nPath('/work/data.csv').read_text()", os=fs)
    assert result == snapshot('a,b\n1,2\n')


def test_deterministic_across_instances():
    """Two identical FakeLinux instances generate identical file content."""
    fs1 = FakeLinux(hostname='same', distro='alpine-3.21')
    fs2 = FakeLinux(hostname='same', distro='alpine-3.21')
    files1 = {f.path.as_posix(): f.read_content() for f in fs1.files}
    files2 = {f.path.as_posix(): f.read_content() for f in fs2.files}
    assert files1 == files2
    assert fs1.uname() == fs2.uname()


def test_pinned_clock(monty_run: RunMonty):
    """today/now pin the clock sandbox code observes."""
    fs = FakeLinux(
        today=datetime.date(2000, 1, 1),
        now=datetime.datetime(2000, 1, 1, 12, 0, 0),
    )
    result = monty_run(
        'import datetime\ndatetime.date.today(), datetime.datetime.now()',
        os=fs,
    )
    assert result == snapshot((datetime.date(2000, 1, 1), datetime.datetime(2000, 1, 1, 12, 0, 0)))


def test_unmet_calls_default_not_handled(monty_run: RunMonty):
    """Base OSAccess does not answer the identity calls; FakeLinux does."""
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run('import os\nos.uname()', os=OSAccess())
    assert str(exc_info.value) == snapshot("RuntimeError: 'os.uname' is not supported in this environment")

    result = monty_run('import os\nos.uname().sysname', os=FakeLinux())
    assert result == 'Linux'
