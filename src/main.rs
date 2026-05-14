use socket2::{Domain, Socket, Type};
use std::os::fd::AsRawFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream, UnixStream};
use tokio::time::timeout;

fn main() {
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8080);

    let upstreams: Arc<Vec<Arc<str>>> = Arc::new(
        std::env::var("UPSTREAMS")
            .expect("UPSTREAMS env var required (comma-separated UDS paths)")
            .split(',')
            .map(|s| Arc::from(s.trim()))
            .filter(|s: &Arc<str>| !s.is_empty())
            .collect(),
    );
    assert!(
        !upstreams.is_empty(),
        "UPSTREAMS must contain at least one path"
    );
    assert!(
        upstreams.len().is_power_of_two(),
        "UPSTREAMS count must be a power of 2 for optimal scheduling"
    );

    let rr: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0));

    let workers: usize = std::env::var("WORKERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);

    // Spawn one Tokio runtime per worker thread.
    // Each worker gets its own listener with SO_REUSEPORT so the kernel
    // distributes incoming connections across threads.
    let handles: Vec<_> = (0..workers)
        .map(|_| {
            let upstreams = upstreams.clone();
            let rr = rr.clone();
            std::thread::spawn(move || {
                tokio::runtime::Builder::new_current_thread()
                    .enable_io()
                    .enable_time()
                    .build()
                    .expect("failed to build Tokio runtime")
                    .block_on(accept_loop(port, upstreams, rr))
            })
        })
        .collect();

    for h in handles {
        h.join().expect("worker thread panicked");
    }
}

fn make_listener(port: u16) -> std::io::Result<std::net::TcpListener> {
    let sock = Socket::new(Domain::IPV4, Type::STREAM, None)?;
    sock.set_reuse_address(true)?;
    // SO_REUSEPORT via libc — guarantees kernel-level accept distribution.
    unsafe {
        let opt: libc::c_int = 1;
        let fd = sock.as_raw_fd();
        let ret = libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_REUSEPORT,
            &opt as *const _ as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        );
        if ret != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    sock.set_nonblocking(true)?;
    let addr: std::net::SocketAddr = format!("0.0.0.0:{port}").parse().unwrap();
    sock.bind(&addr.into())?;
    sock.listen(4096)?;
    Ok(sock.into())
}

async fn accept_loop(port: u16, upstreams: Arc<Vec<Arc<str>>>, rr: Arc<AtomicUsize>) {
    let listener = match make_listener(port) {
        Ok(l) => TcpListener::from_std(l).expect("TcpListener::from_std"),
        Err(e) => {
            eprintln!("failed to create listener: {}", e);
            std::process::exit(1);
        }
    };

    let len = upstreams.len();
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                stream.set_nodelay(true).ok();
                let idx = rr.fetch_add(1, Ordering::Relaxed) & (len - 1);
                let path = upstreams[idx].clone();
                tokio::spawn(handle_connection(stream, path));
            }
            Err(e) => {
                eprintln!("accept error: {}", e);
            }
        }
    }
}

async fn handle_connection(mut tcp: TcpStream, uds_path: Arc<str>) {
    // Retry connection with exponential backoff (up to 5 attempts)
    let mut uds = None;
    let mut attempts = 0;
    let max_attempts = 5;

    while attempts < max_attempts {
        match timeout(Duration::from_millis(500 * (1 << attempts)), UnixStream::connect(uds_path.as_ref())).await {
            Ok(Ok(s)) => {
                uds = Some(s);
                break;
            }
            Ok(Err(e)) => {
                eprintln!("upstream connect error (attempt {}): {}", attempts + 1, e);
                attempts += 1;
                if attempts < max_attempts {
                    tokio::time::sleep(Duration::from_millis(50 * (1 << attempts))).await;
                }
            }
            Err(_) => {
                eprintln!("upstream connect timeout (attempt {})", attempts + 1);
                attempts += 1;
                if attempts < max_attempts {
                    tokio::time::sleep(Duration::from_millis(50 * (1 << attempts))).await;
                }
            }
        }
    }

    let mut uds = match uds {
        Some(s) => s,
        None => {
            eprintln!("upstream connect failed after {} attempts", max_attempts);
            return;
        }
    };

    // Use Tokio's optimized bidirectional copy.  It uses an 8 KiB
    // buffer per direction (reused across polls) and is the fastest
    // portable way to forward between two streams.
    let res = tokio::io::copy_bidirectional(&mut tcp, &mut uds).await;
    if let Err(e) = res {
        // Most errors here are benign (client disconnect, etc.)
        // Only log unexpected ones at a very low rate if desired.
        let _ = e;
    }
}
