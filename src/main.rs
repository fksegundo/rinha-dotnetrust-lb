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
    let diagnostics = std::env::var("RINHA_LB_DIAG").is_ok_and(|s| s == "1");

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
                    .block_on(accept_loop(port, upstreams, rr, diagnostics))
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
    // SO_REUSEPORT via libc keeps accept distribution in the kernel.
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

async fn accept_loop(
    port: u16,
    upstreams: Arc<Vec<Arc<str>>>,
    rr: Arc<AtomicUsize>,
    diagnostics: bool,
) {
    let std_listener = make_listener(port).expect("failed to bind to port");
    let listener =
        TcpListener::from_std(std_listener).expect("failed to convert to Tokio listener");

    let len = upstreams.len();
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                stream.set_nodelay(true).ok();
                let idx = rr.fetch_add(1, Ordering::Relaxed) & (len - 1);
                tokio::spawn(handle_connection(
                    stream,
                    idx,
                    upstreams.clone(),
                    diagnostics,
                ));
            }
            Err(e) => {
                eprintln!("accept error: {}", e);
            }
        }
    }
}

async fn check_backends(upstreams: &[Arc<str>]) -> bool {
    for path in upstreams {
        match timeout(
            Duration::from_millis(500),
            UnixStream::connect(path.as_ref()),
        )
        .await
        {
            Ok(Ok(_)) => continue,
            _ => return false,
        }
    }
    true
}

async fn handle_connection(
    mut tcp: TcpStream,
    upstream_idx: usize,
    upstreams: Arc<Vec<Arc<str>>>,
    diagnostics: bool,
) {
    let mut buf = [0u8; 4096];
    let mut n = 0;
    let mut headers_complete = false;

    while n < buf.len() && !headers_complete {
        match tcp.read(&mut buf[n..]).await {
            Ok(0) => return,
            Ok(bytes_read) => {
                n += bytes_read;
                headers_complete = find_header_end(&buf[..n]).is_some();
            }
            Err(_) => return,
        }
    }

    if !headers_complete {
        let _ = tcp
            .write_all(b"HTTP/1.1 431 Request Header Fields Too Large\r\nContent-Length: 0\r\n\r\n")
            .await;
        return;
    }

    let is_ready = is_ready_request(&buf[..n]);
    if is_ready {
        let ready = check_backends(&upstreams).await;
        let response = if ready {
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok"
        } else {
            "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n"
        };
        let _ = tcp.write_all(response.as_bytes()).await;
        return;
    }

    let mut uds = match connect_upstream(&upstreams, upstream_idx, diagnostics).await {
        Some(s) => s,
        None => {
            let _ = tcp
                .write_all(b"HTTP/1.1 503 Service Unavailable\r\n\r\n")
                .await;
            return;
        }
    };

    if let Err(e) = uds.write_all(&buf[..n]).await {
        if diagnostics {
            eprintln!("write to backend error: {}", e);
        }
        return;
    }

    if diagnostics {
        if let Err(e) = tokio::io::copy_bidirectional(&mut tcp, &mut uds).await {
            eprintln!("proxy copy error: {}", e);
        }
    } else {
        let _ = tokio::io::copy_bidirectional(&mut tcp, &mut uds).await;
    }
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|pos| pos + 4)
}

fn is_ready_request(buf: &[u8]) -> bool {
    buf.starts_with(b"GET /ready ") || buf.starts_with(b"GET /ready\r\n")
}

async fn connect_upstream(
    upstreams: &[Arc<str>],
    start_idx: usize,
    diagnostics: bool,
) -> Option<UnixStream> {
    let len = upstreams.len();

    for round in 0..2 {
        for offset in 0..len {
            let idx = (start_idx + offset) & (len - 1);
            let path = upstreams[idx].as_ref();

            match timeout(Duration::from_millis(150), UnixStream::connect(path)).await {
                Ok(Ok(stream)) => return Some(stream),
                Ok(Err(e)) if diagnostics => {
                    eprintln!(
                        "upstream connect error path={} round={}: {}",
                        path,
                        round + 1,
                        e
                    );
                }
                Err(_) if diagnostics => {
                    eprintln!("upstream connect timeout path={} round={}", path, round + 1);
                }
                _ => {}
            }
        }

        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    None
}
