#![feature(proc_macro, conservative_impl_trait, generators)]

extern crate websocket;
//extern crate futures;
extern crate futures_await as futures;
extern crate tokio_core;

use futures::prelude::*;

use websocket::message::OwnedMessage;
use websocket::async::Server;

use tokio_core::reactor::Core;
use futures::{Future, Sink, Stream};

type UsualClient = websocket::client::async::Framed<tokio_core::net::TcpStream, websocket::async::MessageCodec<websocket::OwnedMessage>>;

#[async]
fn serve_client(client: UsualClient) -> std::result::Result<(),()> {
    let (mut sink, stream) = client.split();
    #[async]
    for m in stream.map_err(|_|()) {
        if m.is_close() { break; }
        let fwd = match m {
            OwnedMessage::Ping(p) => OwnedMessage::Pong(p),
            OwnedMessage::Pong(_) => continue,
            _ => m,
        };
        sink = await!(sink.send(fwd).map_err(|_|()))?;
    }
    await!(sink.send(OwnedMessage::Close(None)).map_err(|_|()))?;
    Ok(())
}

fn main() {
	let mut core = Core::new().unwrap();
	let handle = core.handle();
	let server = Server::bind("127.0.0.1:9002", &handle).unwrap();

	let str = server.incoming().map_err(|_|());
	
	let f = async_block! {
	    #[async]
	    for (upgrade, addr) in str {
            // accept the request to be a ws connection
            println!("Got a connection from: {}", addr);
            let (client, _headers) = await!(upgrade.accept().map_err(|_|()))?;
            let f = serve_client(client);
            handle.spawn(f.map_err(|_|()).map(|_|()));
        }
        Ok::<(),()>(())
    };

	core.run(f).unwrap();
}
