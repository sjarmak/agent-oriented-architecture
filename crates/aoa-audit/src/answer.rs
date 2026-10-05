use std::io;
use std::process::ChildStdout;
use std::time::Duration;

pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(5);

#[cfg(unix)]
pub(crate) struct Answering {
    pipe: ChildStdout,
    written: Vec<u8>,
}

#[cfg(unix)]
impl Answering {
    pub(crate) fn start(pipe: ChildStdout) -> io::Result<Self> {
        let flags = rustix::fs::fcntl_getfl(&pipe)?;
        rustix::fs::fcntl_setfl(&pipe, flags | rustix::fs::OFlags::NONBLOCK)?;
        Ok(Self {
            pipe,
            written: Vec::new(),
        })
    }

    pub(crate) fn take_written(&mut self) -> io::Result<()> {
        self.read_to_the_end_of_what_is_written().map(|_ended| ())
    }

    pub(crate) fn finish(mut self, within: Duration) -> io::Result<Option<Vec<u8>>> {
        let started = std::time::Instant::now();
        loop {
            if self.read_to_the_end_of_what_is_written()? {
                return Ok(Some(self.written));
            }
            if started.elapsed() >= within {
                return Ok(None);
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    fn read_to_the_end_of_what_is_written(&mut self) -> io::Result<bool> {
        use std::io::Read;

        let mut chunk = [0_u8; 4096];
        loop {
            match self.pipe.read(&mut chunk) {
                Ok(0) => return Ok(true),
                Ok(read) => self.written.extend_from_slice(&chunk[..read]),
                Err(pending) if pending.kind() == io::ErrorKind::WouldBlock => return Ok(false),
                Err(interrupted) if interrupted.kind() == io::ErrorKind::Interrupted => {}
                Err(source) => return Err(source),
            }
        }
    }
}

#[cfg(not(unix))]
pub(crate) struct Answering {
    read: std::sync::mpsc::Receiver<io::Result<Vec<u8>>>,
}

#[cfg(not(unix))]
impl Answering {
    pub(crate) fn start(mut pipe: ChildStdout) -> io::Result<Self> {
        use std::io::Read;

        let (send, read) = std::sync::mpsc::channel();
        std::thread::Builder::new().spawn(move || {
            let mut written = Vec::new();
            let _ = send.send(pipe.read_to_end(&mut written).map(|_| written));
        })?;
        Ok(Self { read })
    }

    pub(crate) fn take_written(&mut self) -> io::Result<()> {
        Ok(())
    }

    pub(crate) fn finish(self, within: Duration) -> io::Result<Option<Vec<u8>>> {
        use std::sync::mpsc::RecvTimeoutError;

        match self.read.recv_timeout(within) {
            Ok(read) => read.map(Some),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(io::ErrorKind::BrokenPipe.into()),
        }
    }
}
