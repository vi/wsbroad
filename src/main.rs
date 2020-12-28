use futures::{StreamExt,SinkExt,FutureExt};

use std::rc::Rc;
use std::cell::{RefCell,Cell};

use slab::Slab;
use std::collections::HashMap;

type Result<T> = std::result::Result<T, anyhow::Error>;

use tungstenite::Message;

type UsualClient = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

type ClientSink = Rc<RefCell< futures::stream::SplitSink<UsualClient, Message>  >>;
type ClientStream = futures::stream::SplitStream<UsualClient>;
type AllClients = Rc<RefCell<Slab<ClientSink>>>;
type Url2Clientset = HashMap<String, AllClients>;

const BUFMSG : usize = 3;
const MAXURLS : usize = 64;

async fn process_client_messages(my_id: usize, sink : ClientSink, mut stream: ClientStream, all: &AllClients) -> Result<()> {
    while let Some(m) = stream.next().await {
        let m = m?;

        if m.is_close() { return Ok(()); }
        let fwd = match m {
            Message::Ping(p) => {
                sink.borrow_mut().send(Message::Pong(p)).await?;
                continue;
            },
            Message::Pong(_) => continue,
            Message::Close(_) => break,
            _ => m,
        };
        
        for (id, i) in all.borrow().iter() {
            if id == my_id { continue; }
            
            let mut b = i.borrow_mut();
            
            // Send if possible, throw away message if not ready
            let _ = b.send(fwd.clone()).now_or_never();
        }
    }

    Ok(())
}

async fn serve_client(client: UsualClient, all:AllClients) -> Result<()> {
    
    let (sink, stream) = client.split();
    let sink : ClientSink = Rc::new(RefCell::new(sink));
    
    
    let my_id : usize = all.borrow_mut().insert(sink.clone());

    
    let ret = process_client_messages(my_id, sink, stream, &all).await;
    
    let sink = all.borrow_mut().remove(my_id);
    sink.borrow_mut().send(Message::Close(None)).await?;
    ret?;

    Ok(())
}


async fn client_accepting_loop(listener: tokio::net::TcpListener) -> Result<()> {

    let mapping : Rc<RefCell<Url2Clientset>> = Rc::new(RefCell::new(HashMap::new()));
    let num_urls : Rc<Cell<usize>> = Rc::new(Cell::new(0));
    
    let mut config = tungstenite::protocol::WebSocketConfig::default();

    config.max_send_queue = Some(BUFMSG);

    loop {
        let (socket, from_addr) = listener.accept().await?;

        let mapping = mapping.clone();
        let num_urls = num_urls.clone();

        tokio::task::spawn_local(async move {
            let mut uri = None;
            let cb = |rq: &tungstenite::handshake::server::Request, rsp| {
                uri = Some(rq.uri().to_string());
                Ok(rsp)
            };
            if let Ok(client) = tokio_tungstenite::accept_hdr_async_with_config(socket, cb, Some(config)).await {
                let url = uri.unwrap_or_else(||"NONE".to_string());

                println!("+ {} -> {}", from_addr, url);

                let clientset : AllClients;
                {
                    let mut map = mapping.borrow_mut();
                    
                    if !map.contains_key(&*url) {
                        if num_urls.get() >= MAXURLS {
                            println!("Rejected");
                            return;
                        }
                    
                        let new_clientset : AllClients = Rc::new(RefCell::new(Slab::new()));
                        map.insert(url.clone(), new_clientset);
                        println!("New URL: {}", url);
                        num_urls.set(num_urls.get() + 1);
                    }

                    clientset = map.get(&*url).unwrap().clone();
                }

                if let Err(e) = serve_client(client, clientset).await {
                    if !e.to_string().contains("Connection closed normally") {
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

#[tokio::main(flavor="current_thread")]
async fn main() -> Result<()> {
    let listen_addr : String;
    
    {
        let a = std::env::args().collect::<Vec<_>>();
        if a.len() != 2 || a[1] == "--help" || a[1] == "-?" {
            println!("Usage: wsbroad listen_ip:listen_port");
            ::std::process::exit(1);
        }
        listen_addr = a[1].clone();
    }


    let listener = tokio::net::TcpListener::bind(&listen_addr).await?;

    let ls = tokio::task::LocalSet::new();
    ls.run_until(client_accepting_loop(listener)).await
}
