use futures::{FutureExt, SinkExt, StreamExt};
use tokio_listener::{Connection, Listener};

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use slab::Slab;
use std::collections::HashMap;

type Result<T> = std::result::Result<T, anyhow::Error>;

use tungstenite::Message;

type UsualClient = tokio_tungstenite::WebSocketStream<Connection>;

type ClientSink = Rc<tokio::sync::Mutex<futures::stream::SplitSink<UsualClient, Message>>>;
type ClientStream = futures::stream::SplitStream<UsualClient>;
type AllClients = Rc<RefCell<Slab<ClientSink>>>;
type Url2Clientset = HashMap<String, AllClients>;

const DEFAULT_MAXURLS: usize = 64;

async fn process_client_messages(
    my_id: usize,
    sink: ClientSink,
    mut stream: ClientStream,
    all: &AllClients,
    flags: &flags::Wsbroad,
) -> Result<()> {
    let mut ids_for_backpressure = vec![];
    while let Some(m) = stream.next().await {
        let m = m?;

        if m.is_close() {
            return Ok(());
        }
        let fwd = match m {
            Message::Ping(p) => {
                sink.lock().await.send(Message::Pong(p)).await?;
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
                    let mut cs_lock = cs.lock().await;
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
            for (id, i) in all.borrow().iter() {
                if !flags.reflexive {
                    if id == my_id {
                        continue;
                    }
                }

                // should be always uncontended when backpressure is not enabled
                let mut cs_lock = i.lock().await;

                let _ = cs_lock.send(fwd.clone()).now_or_never();
            }
        }
    }

    Ok(())
}

async fn serve_client(client: UsualClient, all: AllClients, flags: &flags::Wsbroad) -> Result<()> {
    let (sink, stream) = client.split();
    let sink: ClientSink = Rc::new(tokio::sync::Mutex::new(sink));

    let my_id: usize = all.borrow_mut().insert(sink.clone());

    let ret = process_client_messages(my_id, sink, stream, &all, flags).await;

    let sink = all.borrow_mut().remove(my_id);
    sink.lock().await.send(Message::Close(None)).await?;
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
    use tokio_listener::{ListenerAddress, UnixChmodVariant};

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

            /// The target minimum size of the write buffer to reach before writing the data to the underlying stream.
            /// The default value is 128 KiB, but wsbroad flushes after sending every message, so this may be unrelevant.
            optional --write-buffer-size size_bytes: usize

            /// The max size of the write buffer in bytes. Default is 4 MiB.
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
        pub write_buffer_size: Option<usize>,
        pub max_write_buffer_size: Option<usize>,
        pub max_message_size: Option<usize>,
        pub max_frame_size: Option<usize>,
        pub accept_unmasked_frames: bool,
        pub max_urls: Option<usize>,
        pub reflexive: bool,
        pub backpressure: bool,
        pub backpressure_with_errors: bool,
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
    sopts.nodelay = true;
    sopts.sleep_on_errors = true;

    let mut uopts = tokio_listener::UserOptions::default();
    uopts.unix_listen_unlink = flags.unix_listen_unlink;
    uopts.unix_listen_chmod = flags.unix_listen_chmod;
    uopts.unix_listen_uid = flags.unix_listen_uid;
    uopts.unix_listen_gid = flags.unix_listen_gid;
    uopts.sd_accept_ignore_environment = flags.sd_accept_ignore_environment;

    let mut listener = Listener::bind(&flags.listen_addr, &sopts, &uopts).await?;

    let ls = tokio::task::LocalSet::new();
    ls.run_until(client_accepting_loop(&mut listener, Rc::new(flags)))
        .await
}
