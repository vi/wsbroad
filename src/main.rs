#![feature(proc_macro, conservative_impl_trait, generators)]

extern crate websocket;
//extern crate futures;
extern crate futures_await as futures;
extern crate tokio_core;
extern crate compactmap;

use futures::prelude::*;

use websocket::message::OwnedMessage;
use websocket::async::Server;

use tokio_core::reactor::{Core, Handle};
use futures::{Future, Sink, Stream};

use std::rc::Rc;
use std::cell::{RefCell};

use compactmap::CompactMap;

type UsualClient = websocket::client::async::Framed<tokio_core::net::TcpStream, websocket::async::MessageCodec<websocket::OwnedMessage>>;

type ClientSink = Rc<RefCell<futures::sync::mpsc::Sender<websocket::OwnedMessage>>>;
type AllClients = Rc<RefCell<CompactMap<ClientSink>>>;

#[async]
fn serve_client(handle: Handle, client: UsualClient, all:AllClients) -> Result<(),()> {
    let (mut sink, stream) = client.split();
    let (snd,rcv) = futures::sync::mpsc::channel::<OwnedMessage>(3);
    
    let writer = async_block! {
        #[async]
        for m in rcv {
            let lastone = match &m {
                &OwnedMessage::Close(_) => true,
                _ => false,
            };
            sink = await!(sink.send(m).map_err(|_|()))?;
            if lastone { break; }
        }
        Ok::<(),()>(())
    };
    handle.spawn(writer);
    
    let my_id : usize = all.borrow_mut().insert(Rc::new(RefCell::new(snd.clone())));
    
    #[async]
    for m in stream.map_err(|_|()) {
        if m.is_close() { break; }
        let fwd = match m {
            OwnedMessage::Ping(p) => {
                await!(snd.clone().send(OwnedMessage::Pong(p)).map_err(|_|()))?;
                continue
            },
            OwnedMessage::Pong(_) => continue,
            _ => m,
        };
        use std::ops::Deref;
        for (id, i) in all.borrow().deref().into_iter() {
            if id == my_id { continue; }
            
            // Send if possible, throw away message if not ready
            let mut b = i.borrow_mut();
            if let Ok(_) = b.start_send(fwd.clone()) {
                let _ = b.poll_complete();
            }
        }
    }
    
    if let Some(snd) = all.borrow_mut().take(my_id) {
        if let Ok(snd) = Rc::try_unwrap(snd) {
            let _ = snd.into_inner().send(OwnedMessage::Close(None)).poll();
        }
    }
    Ok(())
}

fn main() {
    let mut core = Core::new().unwrap();
    let handle = core.handle();
    let server = Server::bind("127.0.0.1:9002", &handle).unwrap();

    let str = server.incoming().map_err(|_|());
    
    let all_clients : AllClients = Rc::new(RefCell::new(CompactMap::new()));
    
    let f = async_block! {
        #[async]
        for (upgrade, addr) in str {
            let url = format!("{}", upgrade.request.subject.1);
            println!("+ {} -> {}", addr, url);
            let (addr2, url2) = (addr.clone(), url.clone());
            let (client, _headers) = await!(upgrade.accept().map_err(|_|()))?;
            let f = serve_client(handle.clone(), client, all_clients.clone());
            handle.spawn(f.map_err(|_|()).map(move |_|{
                println!("- {} -> {}", addr2, url2);
            }));
        }
        Ok::<(),()>(())
    };

    core.run(f).unwrap();
}
