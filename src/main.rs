use futures::{FutureExt, SinkExt, StreamExt};
use rand::{SeedableRng, Rng};
use tokio_listener::{Connection, Listener};

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use slab::Slab;
use std::collections::HashMap;

type Result<T> = std::result::Result<T, anyhow::Error>;

use tungstenite::Message;

type UsualClient = tokio_tungstenite::WebSocketStream<Connection>;

type ClientStream = futures::stream::SplitStream<UsualClient>;
type AllClients = Rc<RefCell<Slab<ClientSink>>>;
type Url2Clientset = HashMap<String, AllClients>;

#[derive(Clone)]
struct ClientSink {
    s: Rc<tokio::sync::Mutex<futures::stream::SplitSink<UsualClient, Message>>>,
    queue: Option<Rc<tokio::sync::mpsc::Sender<Message>>>,
}

const DEFAULT_MAXURLS: usize = 64;

async fn process_client_messages(
    my_id: usize,
    sink: ClientSink,
    mut stream: ClientStream,
    all: &AllClients,
    flags: &flags::Wsbroad,
) -> Result<()> {
    let mut ids_for_backpressure = vec![];
    let mut rng = flags.stochastic_queue.map(|_|rand::rngs::SmallRng::from_entropy());
    while let Some(m) = stream.next().await {
        let m = m?;

        if m.is_close() {
            return Ok(());
        }
        let fwd = match m {
            Message::Ping(p) => {
                sink.s.lock().await.send(Message::Pong(p)).await?;
                continue;
            }
            Message::Pong(_) => continue,
            Message::Close(_) => break,
            _ => m,
        };

        if flags.backpressure || flags.backpressure_with_errors {
            ids_for_backpressure.clear();
            ids_for_backpressure.extend(
                all.borrow()
                    .iter()
                    .map(|(id, _)| id)
                    .filter(|id| flags.reflexive || *id != my_id),
            );

            for &id in &ids_for_backpressure {
                let all_borrowed = all.borrow();
                if let Some(cs) = all_borrowed.get(id).cloned() {
                    drop(all_borrowed);
                    let mut cs_lock = cs.s.lock().await;

                    let ret = cs_lock.send(fwd.clone()).await;
                    drop(cs_lock);

                    if flags.backpressure_with_errors {
                        ret?;
                    }
                } else {
                    // client gone
                }
            }
        } else {
            // must not await inside this loop
            for (id, i) in all.borrow().iter() {
                if !flags.reflexive {
                    if id == my_id {
                        continue;
                    }
                }

                if let Some(ref queue) = i.queue {
                    // stochastic queue mode

                    if queue.max_capacity() == 1 {
                        let _ = queue.try_send(fwd.clone());
                    } else {
                        let mut denominator = queue.max_capacity() as u32;
                        let mut numerator = queue.capacity() as u32;
                        if denominator > 2 {
                            // first half of the queue is a free ride
                            denominator /= 2;
                            numerator = numerator.saturating_sub(denominator);
                            denominator += 1;
                        } else {
                            denominator += 1;
                        }
                        if rng.as_mut().unwrap().gen_ratio(numerator, denominator) {
                            let _ = queue.try_send(fwd.clone());
                        } else {
                            // actively drop message
                        }
                    }
                } else {
                    // should be always uncontended when backpressure is not enabled and qlen is not used
                    if let Ok(mut cs_lock) = i.s.try_lock() {
                        let _ = cs_lock.send(fwd.clone()).now_or_never();
                        drop(cs_lock);
                    }
                }
            }
        }
    }

    Ok(())
}

async fn serve_client(client: UsualClient, all: AllClients, flags: &flags::Wsbroad) -> Result<()> {
    let (ws_write_part, ws_read_part) = client.split();

    let s = Rc::new(tokio::sync::Mutex::new(ws_write_part));
    let mut sink = ClientSink {
        s,
        queue: None,
    };  

    let jh = if let Some(slen) = flags.stochastic_queue {
        let (tx, mut rx) = tokio::sync::mpsc::channel(slen);
        let s2 = sink.s.clone();
        let jh = tokio::task::spawn_local(async move {
            while let Some(msg) = rx.recv().await {
                if s2.lock().await.send(msg).await.is_err() {
                    break;
                }
            }
        });
        sink.queue = Some(Rc::new(tx));
        Some(jh)
    } else {
        None
    };

    let my_id: usize = all.borrow_mut().insert(sink.clone());

    let ret = process_client_messages(my_id, sink, ws_read_part, &all, flags).await;

    if let Some(jh) = jh {
        jh.abort();
        let _ = jh.await;
    }

    let sink = all.borrow_mut().remove(my_id);
    sink.s.lock().await.send(Message::Close(None)).await?;
    ret?;

    Ok(())
}

async fn client_accepting_loop(listener: &mut Listener, flags: Rc<flags::Wsbroad>) -> Result<()> {
    let mapping: Rc<RefCell<Url2Clientset>> = Rc::new(RefCell::new(HashMap::new()));
    let num_urls: Rc<Cell<usize>> = Rc::new(Cell::new(0));

    let mut config = tungstenite::protocol::WebSocketConfig::default();

    config.write_buffer_size = flags.write_buffer_size.unwrap_or(128 * 1024);
    config.max_write_buffer_size = flags.max_write_buffer_size.unwrap_or(4 * 1024 * 1024);
    config.max_message_size = Some(flags.max_message_size.unwrap_or(1 * 1024 * 1024));
    config.max_frame_size = Some(flags.max_frame_size.unwrap_or(1 * 1024 * 1024));
    config.accept_unmasked_frames = flags.accept_unmasked_frames;

    if config.write_buffer_size + 50 > config.max_write_buffer_size {
        // prevent deadlocks for some reason
        config.write_buffer_size = config.max_write_buffer_size.saturating_sub(50);
    }

    let max_urls = flags.max_urls.unwrap_or(DEFAULT_MAXURLS);

    loop {
        let (socket, from_addr) = listener.accept().await?;

        let mapping = mapping.clone();
        let num_urls = num_urls.clone();

        let flags = flags.clone();
        tokio::task::spawn_local(async move {
            let mut uri = None;
            let cb = |rq: &tungstenite::handshake::server::Request, rsp| {
                uri = Some(rq.uri().to_string());
                Ok(rsp)
            };
            if let Ok(client) =
                tokio_tungstenite::accept_hdr_async_with_config(socket, cb, Some(config)).await
            {
                let url = uri.unwrap_or_else(|| "NONE".to_string());

                println!("+ {} -> {}", from_addr, url);

                let clientset: AllClients;
                {
                    let mut map = mapping.borrow_mut();

                    if !map.contains_key(&*url) {
                        if num_urls.get() >= max_urls {
                            println!("Rejected");
                            return;
                        }

                        let new_clientset: AllClients = Rc::new(RefCell::new(Slab::new()));
                        map.insert(url.clone(), new_clientset);
                        println!("New URL: {}", url);
                        num_urls.set(num_urls.get() + 1);
                    }

                    clientset = map.get(&*url).unwrap().clone();
                }

                if let Err(e) = serve_client(client, clientset, &*flags).await {
                    if !e
                        .to_string()
                        .contains("Trying to work with closed connection")
                    {
                        println!("Error serving client: {}", e);
                    }
                }

                {
                    let mut map = mapping.borrow_mut();

                    let mut do_remove = false;
                    if let Some(x) = map.get(&*url) {
                        if (*x).borrow().len() == 0 {
                            println!("Expiring URL: {}", url);
                            do_remove = true;
                        }
                    }
                    if do_remove {
                        map.remove(&*url);
                        num_urls.set(num_urls.get() - 1);
                    }
                }

                println!("- {} -> {}", from_addr, url);
            } else {
                println!("Failed WebSocket connection attempt from {}", from_addr);
            }
        });
    }
}

mod flags {
    use tokio_listener::{ListenerAddress, TcpKeepaliveParams, UnixChmodVariant};

    xflags::xflags! {
        src "./src/main.rs"

        cmd wsbroad {
            /// TCP or other socket socket address to bind and listen for incoming WebSocket connections
            ///
            /// Specify `sd-listen` for socket-activated mode, file path for UNIX socket (start abstrat addresses with @).
            required listen_addr: ListenerAddress

            /// remove UNIX socket prior to binding to it
            optional --unix-listen-unlink

            /// change filesystem mode of the newly bound UNIX socket to `owner` (006), `group` (066) or `everybody` (666)
            optional --unix-listen-chmod  mode: UnixChmodVariant

            /// change owner user of the newly bound UNIX socket to this numeric uid
            optional --unix-listen-uid uid: u32

            /// change owner group of the newly bound UNIX socket to this numeric uid
            optional  --unix-listen-gid uid: u32

            /// ignore environment variables like LISTEN_PID or LISTEN_FDS and unconditionally use file descritor `3` as a socket in
            /// sd-listen or sd-listen-unix modes
            optional  --sd-accept-ignore-environment

            // set SO_KEEPALIVE settings for each accepted TCP connection. Value is a colon-separated triplet of time_ms:count:interval_ms, each of which is optional.
            optional --tcp-keepalive ka_triplet: TcpKeepaliveParams

            /// try to set SO_REUSEPORT, so that multiple processes can accept connections from the same port in a round-robin fashion.
            ///
            /// Obviously, URL domains would be different based on which instance does the client land.
            optional --tcp-reuse-port

            /// set socket's IPV6_V6ONLY to true, to avoid receiving IPv4 connections on IPv6 socket
            optional --tcp-only-v6

            /// Maximum number of pending unaccepted connections
            optional --tcp-listen-backlog bl : u32

            /// Set size of socket receive buffer size
            optional --recv-buffer-size sz: usize

            /// Set size of socket send buffer size in operating system.
            /// Together with --max-write-buffer-size, it may affect latency when
            /// messages need to be dropped on overload
            optional  --send-buffer-size sz: usize

            /// The target minimum size of the in-app write buffer to reach before writing the data to the underlying stream.
            /// The default value is 128 KiB, but wsbroad flushes after sending every message, so this may be unrelevant.
            ///
            /// May be 0. Needs to be less that --max-write-buffer-size.
            optional --write-buffer-size size_bytes: usize

            /// The max size of the in-app write buffer in bytes. Default is 4 MiB.
            ///
            /// This affects how much messages get buffered before droppign
            /// or slowing down sender begins. Note that --send-buffer-size also affects
            /// this behaviour.
            ///
            /// Also indirectly affects max message size
            optional --max-write-buffer-size size_bytes: usize

            /// The maximum size of a message. Default is 1 MiB.
            optional --max-message-size size_bytes: usize

            /// The maximum size of a single message frame. Default is 1 MiB.
            optional --max-frame-size size_bytes: usize

            // When set, the server will accept and handle unmasked frames from the client (ignoring the RFC requirement).
            optional --accept-unmasked-frames

            /// Maximum number of URLs to handle before rejecting the new ones
            optional --max-urls num : usize

            /// Also send messages back to the sender
            optional --reflexive

            /// Slow down senders if there are active receives that are
            /// unable to take all the messages fast enough.
            /// Makes messages reliable.
            optional --backpressure

            /// Similar to --backpressure, but also disconnect senders to an URL if
            /// we detected that some receiver is abruptly gone.
            ///
            /// Abruptly means we detected an error when trying to send some data to the
            /// client's socket, not to receive from it.
            optional --backpressure-with-errors

            /// Set TCP_NODELAY to deliver small messages with less latency
            optional --nodelay

            /// drop messages to slow receivers not in clusters (i.e. multiple dropped messages in a row),
            /// but with increasing probability based on congestion level.
            /// Value is maximum additional queue length. The bigger - the more uniformly message going to be
            /// dropped when overloaded, but the higher there may be latency for message that go though
            /// 
            /// Short queue descreases thgouhput.
            ///
            /// Note that other buffers (--max-write-buffer-size and --send-buffer-size) still apply after this queue.
            /// 
            /// Unlike other options, the unit is messages, not bytes
            optional --stochastic-queue qlen: usize
        }
    }
    // generated start
    // The following code is generated by `xflags` macro.
    // Run `env UPDATE_XFLAGS=1 cargo build` to regenerate.
    #[derive(Debug)]
    pub struct Wsbroad {
        pub listen_addr: ListenerAddress,

        pub unix_listen_unlink: bool,
        pub unix_listen_chmod: Option<UnixChmodVariant>,
        pub unix_listen_uid: Option<u32>,
        pub unix_listen_gid: Option<u32>,
        pub sd_accept_ignore_environment: bool,
        pub tcp_keepalive: Option<TcpKeepaliveParams>,
        pub tcp_reuse_port: bool,
        pub tcp_only_v6: bool,
        pub tcp_listen_backlog: Option<u32>,
        pub recv_buffer_size: Option<usize>,
        pub send_buffer_size: Option<usize>,
        pub write_buffer_size: Option<usize>,
        pub max_write_buffer_size: Option<usize>,
        pub max_message_size: Option<usize>,
        pub max_frame_size: Option<usize>,
        pub accept_unmasked_frames: bool,
        pub max_urls: Option<usize>,
        pub reflexive: bool,
        pub backpressure: bool,
        pub backpressure_with_errors: bool,
        pub nodelay: bool,
        pub stochastic_queue: Option<usize>,
    }

    impl Wsbroad {
        #[allow(dead_code)]
        pub fn from_env_or_exit() -> Self {
            Self::from_env_or_exit_()
        }

        #[allow(dead_code)]
        pub fn from_env() -> xflags::Result<Self> {
            Self::from_env_()
        }

        #[allow(dead_code)]
        pub fn from_vec(args: Vec<std::ffi::OsString>) -> xflags::Result<Self> {
            Self::from_vec_(args)
        }
    }
    // generated end
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let flags = flags::Wsbroad::from_env_or_exit();

    let mut sopts = tokio_listener::SystemOptions::default();
    sopts.nodelay = flags.nodelay;
    sopts.sleep_on_errors = true;

    let mut uopts = tokio_listener::UserOptions::default();
    uopts.unix_listen_unlink = flags.unix_listen_unlink;
    uopts.unix_listen_chmod = flags.unix_listen_chmod;
    uopts.unix_listen_uid = flags.unix_listen_uid;
    uopts.unix_listen_gid = flags.unix_listen_gid;
    uopts.sd_accept_ignore_environment = flags.sd_accept_ignore_environment;
    uopts.tcp_keepalive = flags.tcp_keepalive;
    uopts.tcp_reuse_port = flags.tcp_reuse_port;
    uopts.tcp_only_v6 = flags.tcp_only_v6;
    uopts.tcp_listen_backlog = flags.tcp_listen_backlog;
    uopts.recv_buffer_size = flags.recv_buffer_size;
    uopts.send_buffer_size = flags.send_buffer_size;

    if flags.stochastic_queue.is_some() && (flags.backpressure || flags.backpressure_with_errors) {
        anyhow::bail!("--stochastic-queue is incompatibel with --backpressure");
    }

    let mut listener = Listener::bind(&flags.listen_addr, &sopts, &uopts).await?;

    let ls = tokio::task::LocalSet::new();
    ls.run_until(client_accepting_loop(&mut listener, Rc::new(flags)))
        .await
}
