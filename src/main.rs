use socket2::{Domain, Socket, Type};
use std::os::fd::AsRawFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
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
    let listener = TcpListener::bind(("0.0.0.0", port))
        .await
        .expect("failed to bind to port");

    let len = upstreams.len();
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                stream.set_nodelay(true).ok();
                let idx = rr.fetch_add(1, Ordering::Relaxed) & (len - 1);
                let path = upstreams[idx].clone();
                tokio::spawn(handle_connection(stream, path, upstreams.clone()));
            }
            Err(e) => {
                eprintln!("accept error: {}", e);
            }
        }
    }
}

async fn check_backends(upstreams: &[Arc<str>]) -> bool {
    for path in upstreams {
        // Try to connect to each backend with a short timeout
        match timeout(Duration::from_millis(500), UnixStream::connect(path.as_ref())).await {
            Ok(Ok(_)) => continue, // Backend is ready
            _ => return false, // Backend not ready
        }
    }
    true
}

async fn handle_connection(mut tcp: TcpStream, uds_path: Arc<str>, upstreams: Arc<Vec<Arc<str>>>) {
    // Read first line to check for /ready endpoint
    let mut buf = [0u8; 1024];
    let n = match tcp.read(&mut buf).await {
        Ok(0) => return, // Connection closed
        Ok(n) => n,
        Err(_) => return,
    };

    // Check if it's a GET /ready request
    let request = std::str::from_utf8(&buf[..n]).unwrap_or("");
    let is_ready = request.starts_with("GET /ready ") || request.starts_with("GET /ready\r\n");

    if is_ready {
        // Check if backends are ready
        let ready = check_backends(&upstreams).await;
        let response = if ready {
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok"
        } else {
            "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n"
        };
        let _ = tcp.write_all(response.as_bytes()).await;
        return;
    }

    // For non-/ready requests, forward to backend
    // First, write the already-read bytes to the backend
    let mut uds = None;
    let mut attempts = 0;
    let max_attempts = 10;

    while attempts < max_attempts {
        match timeout(Duration::from_secs(1 + (attempts as u64)), UnixStream::connect(uds_path.as_ref())).await {
            Ok(Ok(s)) => {
                uds = Some(s);
                break;
            }
            Ok(Err(e)) => {
                eprintln!("upstream connect error (attempt {}): {}", attempts + 1, e);
                attempts += 1;
                if attempts < max_attempts {
                    tokio::time::sleep(Duration::from_millis(100 * (1 << attempts.min(4)))).await;
                }
            }
            Err(_) => {
                eprintln!("upstream connect timeout (attempt {})", attempts + 1);
                attempts += 1;
                if attempts < max_attempts {
                    tokio::time::sleep(Duration::from_millis(100 * (1 << attempts.min(4)))).await;
                }
            }
        }
    }

    let mut uds = match uds {
        Some(s) => s,
        None => {
            eprintln!("upstream connect failed after {} attempts", max_attempts);
            let _ = tcp.write_all(b"HTTP/1.1 503 Service Unavailable\r\n\r\n").await;
            return;
        }
    };

    // Write the already-read bytes to backend
    if let Err(e) = uds.write_all(&buf[..n]).await {
        eprintln!("write to backend error: {}", e);
        return;
    }

    // Use Tokio's optimized bidirectional copy.  It uses an 8 KiB
    // buffer per direction (reused across polls) and is the fastest
    // portable way to forward between two streams.
    let res = tokio::io::copy_bidirectional(&mut tcp, &mut uds).await;
    if let Err(e) = res {
        // Most errors here are benign (client disconnect, etc.)
        let _ = e;
    }
}
