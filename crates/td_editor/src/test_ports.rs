//! The port for tests that need a connect to be refused. A connect to a port that was just freed by dropping a
//! listener can be given that same port as its local port, which connects the socket to itself, so the "dead" server
//! answers with the request it was sent. Nothing can listen on port 0 and no socket has it as its local port, so a
//! connect to it fails at once.
pub const CLOSED: u16 = 0;

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::time::{Duration, Instant};

    #[test]
    fn the_closed_port_is_refused_at_once_and_cannot_be_listened_on() {
        let addr = SocketAddr::from(([127, 0, 0, 1], CLOSED));
        let started = Instant::now();
        let e = TcpStream::connect_timeout(&addr, Duration::from_secs(5)).unwrap_err();
        assert_ne!(e.kind(), std::io::ErrorKind::TimedOut, "{e}");
        assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());

        let taken: Vec<u16> = (0..20).map(|_| TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()).collect();
        assert!(taken.iter().all(|&p| p != CLOSED), "{taken:?}");
    }
}
