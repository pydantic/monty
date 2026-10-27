//! Pool-owned CPU timers. Only the watcher thread receives their signals.

use std::{
    collections::HashMap,
    io::{self, Read, Write},
    os::{fd::AsFd, unix::net::UnixStream},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use nix::{
    errno::Errno,
    libc::time_t,
    poll::{PollFd, PollFlags, poll},
    sys::{
        signal::{SigEvent, SigSet, SigevNotify, Signal},
        signalfd::{SfdFlags, SignalFd},
        timer::{Expiration, Timer, TimerSetTimeFlags},
    },
    time::ClockId,
    unistd::gettid,
};
use tokio::sync::oneshot;

#[derive(Clone)]
pub(crate) struct Driver(Arc<Endpoint>);

struct Endpoint {
    commands: mpsc::Sender<Command>,
    wake: UnixStream,
    next_id: AtomicUsize,
    thread: Option<JoinHandle<()>>,
}

enum Command {
    Arm {
        id: usize,
        clock: ClockId,
        deadline: Duration,
        result: oneshot::Sender<io::Result<()>>,
    },
    Cancel(usize),
    Stop,
}

struct Entry {
    timer: Timer,
    result: oneshot::Sender<io::Result<()>>,
}

struct Registration {
    endpoint: Arc<Endpoint>,
    id: usize,
    result: oneshot::Receiver<io::Result<()>>,
}

impl Driver {
    pub(crate) fn new() -> io::Result<Self> {
        let (wake, read) = UnixStream::pair()?;
        wake.set_nonblocking(true)?;
        read.set_nonblocking(true)?;
        let (commands, receiver) = mpsc::channel();
        let (ready, startup) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("monty-cpu-timers".into())
            .spawn(move || match signal_fd() {
                Ok(signals) => {
                    let _ = ready.send(Ok(()));
                    watch(read, &signals, &receiver);
                }
                Err(error) => {
                    let _ = ready.send(Err(error));
                }
            })?;
        let endpoint = Endpoint {
            commands,
            wake,
            next_id: AtomicUsize::new(1),
            thread: Some(thread),
        };
        startup.recv().map_err(io::Error::other)??;
        Ok(Self(Arc::new(endpoint)))
    }

    pub(crate) async fn expired(&self, clock: ClockId, deadline: Duration) -> io::Result<()> {
        let id = self
            .0
            .next_id
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| io::Error::other("CPU timer registration IDs exhausted"))?;
        let (result, receiver) = oneshot::channel();
        self.0.send(Command::Arm {
            id,
            clock,
            deadline,
            result,
        })?;
        let mut registration = Registration {
            endpoint: Arc::clone(&self.0),
            id,
            result: receiver,
        };
        (&mut registration.result).await.map_err(io::Error::other)?
    }
}

impl Endpoint {
    fn send(&self, command: Command) -> io::Result<()> {
        self.commands.send(command).map_err(io::Error::other)?;
        loop {
            match (&self.wake).write(&[1]) {
                Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "CPU timer wake socket closed")),
                Ok(_) => return Ok(()),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) => return Err(error),
            }
        }
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        let _ = self.endpoint.send(Command::Cancel(self.id));
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        let _ = self.send(Command::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn signal_fd() -> io::Result<SignalFd> {
    let mut mask = SigSet::empty();
    mask.add(Signal::SIGUSR1);
    mask.thread_block()?;
    SignalFd::with_flags(&mask, SfdFlags::SFD_NONBLOCK | SfdFlags::SFD_CLOEXEC).map_err(Into::into)
}

fn watch(mut wake: UnixStream, signals: &SignalFd, commands: &mpsc::Receiver<Command>) {
    let mut active = HashMap::<usize, Entry>::new();
    if let Err(error) = watch_loop(&mut wake, signals, commands, &mut active) {
        let message = error.to_string();
        for (_, entry) in active {
            let _ = entry.result.send(Err(io::Error::other(message.clone())));
        }
    }
}

fn watch_loop(
    wake: &mut UnixStream,
    signals: &SignalFd,
    commands: &mpsc::Receiver<Command>,
    active: &mut HashMap<usize, Entry>,
) -> io::Result<()> {
    loop {
        let mut fds = [
            PollFd::new(wake.as_fd(), PollFlags::POLLIN),
            PollFd::new(signals.as_fd(), PollFlags::POLLIN),
        ];
        match poll(&mut fds, None::<u16>) {
            Ok(_) => {}
            Err(Errno::EINTR) => continue,
            Err(error) => return Err(error.into()),
        }
        let mut bytes = [0; 256];
        loop {
            match wake.read(&mut bytes) {
                Ok(0) => return Ok(()),
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(error),
            }
        }
        for command in commands.try_iter() {
            match command {
                Command::Arm {
                    id,
                    clock,
                    deadline,
                    result,
                } => match arm(clock, deadline) {
                    Ok(timer) => {
                        active.insert(id, Entry { timer, result });
                    }
                    Err(error) => {
                        let _ = result.send(Err(error));
                    }
                },
                Command::Cancel(id) => {
                    active.remove(&id);
                }
                Command::Stop => return Ok(()),
            }
        }
        let mut notified = false;
        while signals.read_signal()?.is_some() {
            notified = true;
        }
        if notified {
            // Standard signals coalesce. One wake must collect every expired timer.
            let expired: Vec<_> = active
                .iter()
                .filter_map(|(&id, entry)| match entry.timer.get() {
                    Ok(Some(_)) => None,
                    result => Some((id, result.map(|_| ()).map_err(io::Error::from))),
                })
                .collect();
            for (id, result) in expired {
                if let Some(entry) = active.remove(&id) {
                    let _ = entry.result.send(result);
                }
            }
        }
    }
}

fn arm(clock: ClockId, deadline: Duration) -> io::Result<Timer> {
    time_t::try_from(deadline.as_secs()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "CPU deadline exceeds the kernel clock range",
        )
    })?;
    let event = SigEvent::new(SigevNotify::SigevThreadId {
        signal: Signal::SIGUSR1,
        thread_id: gettid().as_raw(),
        si_value: 0,
    });
    let mut timer = Timer::new(clock, event)?;
    timer.set(
        Expiration::OneShot(deadline.into()),
        TimerSetTimeFlags::TFD_TIMER_ABSTIME,
    )?;
    Ok(timer)
}

#[cfg(test)]
#[path = "../tests/unit/cpu_timer.rs"]
mod tests;
