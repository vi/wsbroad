#![feature(proc_macro, conservative_impl_trait, generators)]

extern crate websocket;
//extern crate futures;
extern crate futures_await as futures;
extern crate tokio_core;

use futures::prelude::*;

use websocket::message::OwnedMessage;
use websocket::async::Server;

use tokio_core::reactor::{Core, Handle};
use futures::{Future, Sink, Stream};

use std::rc::Rc;
use std::cell::{RefCell};

type UsualClient = websocket::client::async::Framed<tokio_core::net::TcpStream, websocket::async::MessageCodec<websocket::OwnedMessage>>;

type ClientSink = futures::sync::mpsc::Sender<websocket::OwnedMessage>;
type AllClients = Rc<RefCell<Vec<ClientSink>>>;

#[async]
fn serve_client(handle: Handle, client: UsualClient, all:AllClients) -> Result<(),()> {
    let (mut sink, stream) = client.split();
    let (snd,rcv) = futures::sync::mpsc::channel::<OwnedMessage>(1);
    
    let writer = async_block! {
        #[async]
        for m in rcv {
            sink = await!(sink.send(m).map_err(|_|()))?;
        }
        Ok::<(),()>(())
    };
    handle.spawn(writer);
    
    all.borrow_mut().push(snd.clone());
    
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
        for i in all.borrow().iter() {
            let mut q : ClientSink = i.clone();
            // Send if possible, throw away message if not ready
            if let Ok(_) = q.start_send(fwd.clone()) {
                let _ = q.poll_complete();
            }
        }
    }
    // do not close
    Ok(())
}

fn main() {
    let mut core = Core::new().unwrap();
    let handle = core.handle();
    let server = Server::bind("127.0.0.1:9002", &handle).unwrap();

    let str = server.incoming().map_err(|_|());
    
    let all_clients : AllClients = Rc::new(RefCell::new(vec![]));
    
    let f = async_block! {
        #[async]
        for (upgrade, addr) in str {
            println!("Got a connection from: {}", addr);
            let (client, _headers) = await!(upgrade.accept().map_err(|_|()))?;
            let f = serve_client(handle.clone(), client, all_clients.clone());
            handle.spawn(f.map_err(|_|()).map(|_|()));
        }
        Ok::<(),()>(())
    };

    core.run(f).unwrap();
}
