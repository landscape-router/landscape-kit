use std::collections::HashMap;

use landscape_terrain_proto::ipstack::{IpStack, SocketHandle, StackMsg};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};

use super::{ClientEvent, Forward, ForwardStatus, LogLevel, LogSink, emit_event};

pub(super) type ConnKey = (SocketHandle, u64);

pub(super) struct Conn {
    pub(super) from_tx: mpsc::Sender<Vec<u8>>,
    pub(super) forward: Forward,
    /// Source port used by the internal TCP connection. It stays reserved
    /// while the smoltcp socket is in TIME-WAIT so a later mapping cannot
    /// reuse the same four-tuple prematurely.
    pub(super) local_port: u16,
    /// Generation prevents a delayed task message from targeting a later
    /// connection that reuses the same smoltcp socket handle.
    pub(super) generation: u64,
    pub(super) close_tx: Option<oneshot::Sender<()>>,
}

pub(super) fn spawn_listener(
    (listen_port, dst_port): Forward,
    accept_tx: mpsc::Sender<(TcpStream, Forward)>,
    log: LogSink,
    events: Option<mpsc::UnboundedSender<ClientEvent>>,
) -> tokio::task::JoinHandle<()> {
    emit_event(
        &events,
        ClientEvent::ForwardStatus {
            forward: (listen_port, dst_port),
            status: ForwardStatus::Starting,
        },
    );
    tokio::spawn(async move {
        run_listener(listen_port, dst_port, accept_tx, log, events).await;
    })
}

pub(super) fn close_connections(
    forward: Forward,
    stack: &mut IpStack,
    conns: &mut HashMap<SocketHandle, Conn>,
) {
    let handles: Vec<SocketHandle> = conns
        .iter()
        .filter_map(|(handle, conn)| (conn.forward == forward).then_some(*handle))
        .collect();
    for handle in handles {
        if let Some(conn) = conns.get_mut(&handle)
            && let Some(close_tx) = conn.close_tx.take()
        {
            let _ = close_tx.send(());
        }
        stack.close_socket(handle);
    }
}

pub(super) async fn bridge_task(
    mut stream: TcpStream,
    key: ConnKey,
    to_tx: mpsc::Sender<(ConnKey, StackMsg)>,
    mut from_rx: mpsc::Receiver<Vec<u8>>,
    mut close_rx: oneshot::Receiver<()>,
) {
    let mut buf = vec![0u8; 8192];
    loop {
        tokio::select! {
            read = stream.read(&mut buf) => {
                match read {
                    Ok(0) => {
                        signal_close(&to_tx, key).await;
                        return;
                    }
                    Ok(count) => {
                        if to_tx
                            .send((key, StackMsg::Data(buf[..count].to_vec())))
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(_) => {
                        signal_close(&to_tx, key).await;
                        return;
                    }
                }
            }
            message = from_rx.recv() => {
                match message {
                    Some(bytes) => {
                        if stream.write_all(&bytes).await.is_err() {
                            signal_close(&to_tx, key).await;
                            return;
                        }
                    }
                    None => {
                        signal_close(&to_tx, key).await;
                        return;
                    }
                }
            }
            _ = &mut close_rx => {
                let _ = stream.shutdown().await;
                signal_close(&to_tx, key).await;
                return;
            }
        }
    }
}

async fn signal_close(to_tx: &mpsc::Sender<(ConnKey, StackMsg)>, key: ConnKey) {
    let _ = to_tx.send((key, StackMsg::Close)).await;
}

async fn run_listener(
    listen_port: u16,
    dst_port: u16,
    accept_tx: mpsc::Sender<(TcpStream, Forward)>,
    log: LogSink,
    events: Option<mpsc::UnboundedSender<ClientEvent>>,
) {
    let listener = match TcpListener::bind(("127.0.0.1", listen_port)).await {
        Ok(listener) => listener,
        Err(error) => {
            emit_event(
                &events,
                ClientEvent::ForwardStatus {
                    forward: (listen_port, dst_port),
                    status: ForwardStatus::Failed,
                },
            );
            log.emit(
                LogLevel::Warn,
                format!("  cannot listen on 127.0.0.1:{listen_port}: {error}"),
            );
            return;
        }
    };
    emit_event(
        &events,
        ClientEvent::ForwardStatus {
            forward: (listen_port, dst_port),
            status: ForwardStatus::Listening,
        },
    );
    log.emit(
        LogLevel::Info,
        format!("  forward: 127.0.0.1:{listen_port} -> router 127.0.0.1:{dst_port}"),
    );
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                if accept_tx
                    .send((stream, (listen_port, dst_port)))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            Err(error) => log.emit(
                LogLevel::Warn,
                format!("  accept error on {listen_port}: {error}"),
            ),
        }
    }
}
