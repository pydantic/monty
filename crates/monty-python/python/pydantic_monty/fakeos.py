"""A deterministic fake Linux system for Monty sandboxes.

[`FakeLinux`] subclasses [`OSAccess`][pydantic_monty.OSAccess] with a fully
synthetic, reproducible Linux machine: `/etc`, `/proc`, `/dev`, `/home`, fake
`environ`, and host-answered `os.uname()` / `os.getcwd()` / `os.cpu_count()` /
`os.getpid()` / `os.system()`. Sandboxed code sees a convincing Linux box; the
host sees a pure in-memory object with nothing real behind it.

Everything is deterministic by construction — the same constructor arguments
produce byte-identical files and answers on every host and every run, which
makes snapshots, tests and evals stable.
"""

from __future__ import annotations

import datetime
from pathlib import PurePosixPath
from typing import Callable, Sequence

from .os_access import AbstractFile, MemoryFile, OSAccess, UnameResult

__all__ = ('FakeLinux',)

_DISTRO_PRESETS: dict[str, dict[str, str]] = {
    'ubuntu-24.04': {
        'NAME': 'Ubuntu',
        'VERSION_ID': '24.04',
        'ID': 'ubuntu',
        'PRETTY_NAME': 'Ubuntu 24.04.1 LTS',
        'VERSION': '24.04.1 LTS (Noble Numbat)',
        'VERSION_CODENAME': 'noble',
        'ID_LIKE': 'debian',
        'HOME_URL': 'https://www.ubuntu.com/',
    },
    'debian-12': {
        'NAME': 'Debian GNU/Linux',
        'VERSION_ID': '12',
        'ID': 'debian',
        'PRETTY_NAME': 'Debian GNU/Linux 12 (bookworm)',
        'VERSION_CODENAME': 'bookworm',
        'ID_LIKE': 'debian',
        'HOME_URL': 'https://www.debian.org/',
    },
    'alpine-3.21': {
        'NAME': 'Alpine Linux',
        'VERSION_ID': '3.21.0',
        'ID': 'alpine',
        'PRETTY_NAME': 'Alpine Linux v3.21',
        'ID_LIKE': 'busybox musl',
        'HOME_URL': 'https://www.alpinelinux.org/',
    },
    'fedora-41': {
        'NAME': 'Fedora Linux',
        'VERSION_ID': '41',
        'ID': 'fedora',
        'PRETTY_NAME': 'Fedora Linux 41 (Container Image)',
        'PLATFORM_ID': 'platform:f41',
        'HOME_URL': 'https://fedoraproject.org/',
    },
}

# Directories that exist on every Linux box but contain no generated files.
# The in-memory tree materializes directories only as file parents, so these
# are injected as empty entries after construction.
_SHELL_PATHS = (
    '/bin',
    '/boot',
    '/dev',
    '/etc',
    '/home',
    '/lib',
    '/media',
    '/mnt',
    '/opt',
    '/proc',
    '/root',
    '/run',
    '/sbin',
    '/srv',
    '/sys',
    '/tmp',
    '/usr',
    '/usr/bin',
    '/usr/lib',
    '/usr/local',
    '/usr/local/bin',
    '/usr/sbin',
    '/usr/share',
    '/var',
    '/var/log',
    '/var/tmp',
)

# Executables /usr/bin advertises. Real binaries obviously aren't included —
# they are empty regular files so `is_file()` and directory listings feel right.
_USR_BIN_TOOLS = ('awk', 'bash', 'cat', 'env', 'grep', 'ls', 'python3', 'sed', 'sh', 'uname')


class FakeLinux(OSAccess):
    """An in-memory Linux machine: filesystem, environment and system identity.

    Subclass of [`OSAccess`][pydantic_monty.OSAccess] that generates a
    synthetic-but-convincing Linux tree and answers the system-identity `os`
    calls deterministically. Use it anywhere an `os=` handler is accepted::

        from pydantic_monty import Monty
        from pydantic_monty.fakeos import FakeLinux

        fs = FakeLinux(hostname='prod-web-1', distro='debian-12')

        with Monty() as pool:
            with pool.checkout() as session:
                session.feed_run('import os\\nprint(os.uname().nodename)', os=fs)
                # > prod-web-1

    Determinism: with the same constructor arguments, every generated file,
    `stat`, and identity answer is byte-identical across hosts and runs — no
    host facts (hostname, CPU count, clock, ...) leak in. The one exception is
    the clock, which proxies to the host unless `today` / `now` are pinned.

    `os.system` never executes anything. Commands are recorded in
    [`commands`][pydantic_monty.fakeos.FakeLinux.commands] and answered with
    `0` (or delegated to `system_handler`), so sandboxed code that shells out
    observes success while the host asserts on exactly what was "run".
    """

    commands: list[str]
    """Every command passed to `os.system`, in order — assert against this in
    tests/evals to verify what sandboxed code tried to run."""

    def __init__(
        self,
        *,
        hostname: str = 'monty',
        distro: str | dict[str, str] = 'ubuntu-24.04',
        kernel: str = '6.8.0-45-generic',
        machine: str = 'x86_64',
        cpu_count: int = 8,
        memory_total: int = 16 * 1024**3,
        user: str = 'monty',
        cwd: str = '/',
        pid: int = 1337,
        today: datetime.date | None = None,
        now: datetime.datetime | None = None,
        environ: dict[str, str] | None = None,
        files: Sequence[AbstractFile] | None = None,
        system_handler: Callable[[str], int] | None = None,
    ) -> None:
        """Build the fake system.

        Args:
            hostname: `os.uname().nodename` and `/etc/hostname`.
            distro: A preset name (`ubuntu-24.04`, `debian-12`, `alpine-3.21`,
                `fedora-41`) or a custom `os-release` key/value mapping.
            kernel: `os.uname().release` and `/proc/version`.
            machine: `os.uname().machine`; `x86_64` and `aarch64` get matching
                `/proc/cpuinfo` layouts.
            cpu_count: `os.cpu_count()` and the processors in `/proc/cpuinfo`.
            memory_total: Total RAM in bytes; `/proc/meminfo` derives from it.
            user: The non-root account in `/etc/passwd`; drives `$HOME`, `$USER`.
            cwd: `os.getcwd()` and `$PWD`.
            pid: `os.getpid()` and `/proc/self`.
            today: Pins `date.today()`; the host clock is used when `None`.
            now: Pins `datetime.now()`; the host clock is used when `None`.
            environ: Extra/overriding environment variables on top of the
                generated defaults (`PATH`, `HOME`, `USER`, `SHELL`, ...).
            files: Extra files layered over the generated tree.
            system_handler: Optional hook invoked as `system_handler(command)`
                instead of the default record-and-return-`0` behavior. It runs
                on the host with full authority — only pass trusted handlers.
        """
        self.commands = []
        self._identity = (hostname, distro, kernel, machine, cpu_count, memory_total, user, pid)
        self._cwd = cwd
        self._today = today
        self._now = now
        self._system_handler = system_handler

        release = _DISTRO_PRESETS[distro] if isinstance(distro, str) else dict(distro)
        version = f'Linux version {kernel} (monty@localhost) (gcc 13.2.0) #1 SMP PREEMPT_DYNAMIC'
        meminfo = _meminfo(memory_total)
        cpuinfo = _cpuinfo(machine, cpu_count)
        passwd = _passwd(user)

        generated: list[AbstractFile] = [
            MemoryFile('/etc/os-release', _os_release(release)),
            MemoryFile('/etc/hostname', f'{hostname}\n'),
            MemoryFile('/etc/hosts', f'127.0.0.1 localhost\n127.0.1.1 {hostname}\n'),
            MemoryFile('/etc/resolv.conf', 'nameserver 1.1.1.1\nnameserver 8.8.8.8\n'),
            MemoryFile('/etc/timezone', 'UTC\n'),
            MemoryFile('/etc/passwd', passwd),
            MemoryFile('/etc/group', f'root:x:0:\n{user}:x:1000:\n'),
            MemoryFile('/proc/version', f'{version}\n'),
            MemoryFile('/proc/cpuinfo', cpuinfo),
            MemoryFile('/proc/meminfo', meminfo),
            MemoryFile('/proc/uptime', '86400.00 345600.00\n'),
            MemoryFile('/proc/loadavg', f'0.08 0.12 0.05 1/{120 + pid} {pid}\n'),
            MemoryFile('/proc/self/cmdline', 'python3\x00'),
            MemoryFile('/proc/self/comm', 'python3\n'),
            MemoryFile('/proc/self/status', _proc_status(pid, memory_total)),
            MemoryFile('/var/log/syslog', f'{hostname} kernel: [    0.000000] Linux version {kernel}\n'),
            MemoryFile('/var/log/auth.log', f'{hostname} sshd[1]: Server listening on 0.0.0.0 port 22.\n'),
            MemoryFile('/dev/null', ''),
        ]
        generated.extend(MemoryFile(f'/usr/bin/{name}', '', permissions=0o755) for name in _USR_BIN_TOOLS)
        if files is not None:
            generated.extend(files)

        base_environ = {
            'PATH': '/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin',
            'HOME': f'/home/{user}',
            'USER': user,
            'LOGNAME': user,
            'SHELL': '/bin/bash',
            'LANG': 'C.UTF-8',
            'HOSTNAME': hostname,
            'PWD': cwd,
            'TERM': 'xterm-256color',
        }
        if environ is not None:
            base_environ.update(environ)

        super().__init__(generated, base_environ)
        self._inject_dirs()

    # === structure ==========================================================

    def _inject_dirs(self) -> None:
        """Materialize the empty standard directories in the tree."""
        for path in (*_SHELL_PATHS, f'/home/{self._identity[6]}', '/proc/self/fd'):
            subtree = self._tree
            for part in PurePosixPath(path).parts:
                entry = subtree.setdefault(part, {})
                if not isinstance(entry, dict):
                    raise ValueError(f'Cannot create directory {path}: {part} is a file')
                subtree = entry

    # === system-identity hooks ==============================================

    def uname(self) -> UnameResult:
        """The synthetic `uname_result` sandbox code observes."""
        hostname, _distro, kernel, machine, _cpu_count, _memory_total, _user, _pid = self._identity
        return UnameResult('Linux', hostname, kernel, '#1 SMP PREEMPT_DYNAMIC', machine)

    def getcwd(self) -> str:
        """The virtual working directory sandbox code observes."""
        return self._cwd

    def cpu_count(self) -> int:
        """The synthetic CPU count sandbox code observes."""
        return self._identity[4]

    def getpid(self) -> int:
        """The synthetic process ID sandbox code observes."""
        return self._identity[7]

    def system(self, command: str) -> int:
        """Record the command and return an exit status; never executes.

        With a `system_handler` the command is delegated (and still recorded);
        otherwise the deterministic answer is `0`.
        """
        self.commands.append(command)
        if self._system_handler is not None:
            return self._system_handler(command)
        return 0

    # === clock ==============================================================

    def date_today(self) -> datetime.date:
        """`today` when pinned, else the host clock."""
        return self._today if self._today is not None else super().date_today()

    def datetime_now(self, tz: datetime.tzinfo | None = None) -> datetime.datetime:
        """`now` when pinned, else the host clock."""
        return self._now if self._now is not None else super().datetime_now(tz=tz)

    # === special files ======================================================

    def _write_file(self, path: PurePosixPath, data: bytes | str) -> None:
        if path == PurePosixPath('/dev/null'):
            return
        super()._write_file(path, data)

    def _append(self, path: PurePosixPath, data: bytes | str) -> None:
        if path == PurePosixPath('/dev/null'):
            return
        super()._append(path, data)


# =============================================================================
# Generated-file content builders — pure functions of the constructor knobs.
# =============================================================================


def _os_release(release: dict[str, str]) -> str:
    return ''.join(f'{key}="{value}"\n' for key, value in release.items())


def _passwd(user: str) -> str:
    return (
        'root:x:0:0:root:/root:/bin/bash\n'
        'daemon:x:1:1:daemon:/usr/sbin:/usr/sbin/nologin\n'
        f'{user}:x:1000:1000:{user}:/home/{user}:/bin/bash\n'
    )


def _meminfo(memory_total: int) -> str:
    total_kb = memory_total // 1024
    free_kb = total_kb // 2
    cached_kb = total_kb // 4
    return (
        f'MemTotal:       {total_kb} kB\n'
        f'MemFree:        {free_kb} kB\n'
        f'MemAvailable:   {free_kb + cached_kb} kB\n'
        f'Buffers:        {cached_kb // 2} kB\n'
        f'Cached:         {cached_kb} kB\n'
        'SwapTotal:             0 kB\n'
        'SwapFree:              0 kB\n'
    )


def _cpuinfo(machine: str, cpu_count: int) -> str:
    if machine == 'aarch64':
        return ''.join(
            f'processor\t: {n}\nBogoMIPS\t: 48.00\nFeatures\t: fp asimd evtstrm aes pmull\nCPU implementer\t: 0x41\n'
            f'CPU part\t: 0xd0c\n\n'
            for n in range(cpu_count)
        )
    return ''.join(
        f'processor\t: {n}\n'
        'vendor_id\t: GenuineIntel\n'
        'model name\t: Monty Virtual CPU\n'
        'flags\t\t: fpu vme de pse tsc msr sse sse2 avx avx2\n\n'
        for n in range(cpu_count)
    )


def _proc_status(pid: int, memory_total: int) -> str:
    return (
        f'Name:\tpython3\nState:\tR (running)\nTgid:\t{pid}\nPid:\t{pid}\nPPid:\t{pid - 1}\n'
        f'VmRSS:\t{memory_total // (1024 * 1024)} kB\nThreads:\t1\n'
    )
