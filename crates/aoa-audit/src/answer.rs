use std::io;
use std::process::ChildStdout;
use std::time::{Duration, Instant};

pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(5);

#[cfg(unix)]
const PASS_LIMIT: usize = 16 * 1024;

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

    pub(crate) fn take_written(&mut self, until: Instant) -> io::Result<()> {
        self.read_a_pass(until).map(|_ended| ())
    }

    pub(crate) fn finish(mut self, within: Duration) -> io::Result<Option<Vec<u8>>> {
        let until = Instant::now() + within;
        loop {
            if self.read_a_pass(until)? {
                return Ok(Some(self.written));
            }
            if Instant::now() >= until {
                return Ok(None);
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    fn read_a_pass(&mut self, until: Instant) -> io::Result<bool> {
        use std::io::Read;

        let mut chunk = [0_u8; 4096];
        let mut taken = 0;
        while taken < PASS_LIMIT && Instant::now() < until {
            let room = chunk.len().min(PASS_LIMIT - taken);
            match self.pipe.read(&mut chunk[..room]) {
                Ok(0) => return Ok(true),
                Ok(read) => {
                    self.written.extend_from_slice(&chunk[..read]);
                    taken += read;
                }
                Err(pending) if pending.kind() == io::ErrorKind::WouldBlock => return Ok(false),
                Err(interrupted) if interrupted.kind() == io::ErrorKind::Interrupted => {}
                Err(source) => return Err(source),
            }
        }
        Ok(false)
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

    pub(crate) fn take_written(&mut self, _until: Instant) -> io::Result<()> {
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

#[cfg(all(test, unix))]
mod tests {
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc::channel;

    use super::*;

    const LONG_ENOUGH_TO_FILL_THE_PIPE: Duration = Duration::from_millis(300);

    fn a_producer_that_never_stops() -> (Child, Answering) {
        let mut child = Command::new("yes")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let answering = Answering::start(child.stdout.take().unwrap()).unwrap();
        (child, answering)
    }

    fn stop(mut child: Child) {
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn one_pass_takes_a_bounded_amount_from_a_pipe_that_holds_more() {
        let (child, mut answering) = a_producer_that_never_stops();
        std::thread::sleep(LONG_ENOUGH_TO_FILL_THE_PIPE);

        let taken = answering.take_written(Instant::now() + Duration::from_secs(5));
        stop(child);

        taken.unwrap();
        assert_eq!(answering.written.len(), PASS_LIMIT);
    }

    #[test]
    fn a_pass_begun_at_the_deadline_reads_nothing() {
        let (child, mut answering) = a_producer_that_never_stops();
        std::thread::sleep(LONG_ENOUGH_TO_FILL_THE_PIPE);

        let taken = answering.take_written(Instant::now());
        stop(child);

        taken.unwrap();
        assert_eq!(answering.written.len(), 0);
    }

    #[test]
    fn finishing_gives_up_at_the_deadline_on_a_producer_that_never_stops() {
        let (child, answering) = a_producer_that_never_stops();
        let (send, done) = channel();
        std::thread::spawn(move || {
            let started = Instant::now();
            let finished = answering.finish(Duration::from_millis(200));
            let _ = send.send((finished.map(|written| written.is_some()), started.elapsed()));
        });

        let finished = done.recv_timeout(Duration::from_secs(5));
        stop(child);

        let (answered, took) = finished.expect("the deadline was starved by continuous output");
        assert!(matches!(answered, Ok(false)), "{answered:?}");
        assert!(took < Duration::from_secs(2), "{took:?}");
    }
}
