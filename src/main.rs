#![feature(proc_macro, conservative_impl_trait, generators)]

extern crate websocket;
//extern crate futures;
extern crate futures_await as futures;
extern crate tokio_core;

use futures::prelude::*;

use websocket::message::OwnedMessage;
//use websocket::server::InvalidConnection;
use websocket::async::Server;

use tokio_core::reactor::Core;
use futures::{Future, Sink, Stream};


// thow away error
struct ThaeS<F : futures::stream::Stream>(F);

impl<F:futures::stream::Stream> futures::stream::Stream for ThaeS<F> {
    type Item = F::Item;
    type Error = std::io::Error;
    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        match self.0.poll() {
            Ok(x) => Ok(x),
            Err(_) => Err(std::io::Error::new(std::io::ErrorKind::Other, "...")),
        }
    }
}

trait IntoThaeS<F : futures::stream::Stream> {
    fn thaes(self) -> ThaeS<F>;
}
impl<F:futures::stream::Stream> IntoThaeS<F> for F {
    fn thaes(self) -> ThaeS<F> { ThaeS(self) }
}

// thow away error
struct ThaeF<F : futures::Future>(F);

impl<F:futures::Future> futures::Future for ThaeF<F> {
    type Item = F::Item;
    type Error = std::io::Error;
    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.0.poll() {
            Ok(x) => Ok(x),
            Err(_) => Err(std::io::Error::new(std::io::ErrorKind::Other, "...")),
        }
    }
}

trait IntoThaeF<F : futures::Future> {
    fn thaef(self) -> ThaeF<F>;
}
impl<F:futures::Future> IntoThaeF<F> for F {
    fn thaef(self) -> ThaeF<F> { ThaeF(self) }
}

/*
fn thae<T,R>(x : T) -> R where T:futures::stream::Stream, R:futures::stream::Stream<Item=T::Item, Error=std::io::Error> {
    x.map_err(|_|std::io::Error::new(std::io::ErrorKind::Other, "..."))
}*/

type UsualClient = websocket::client::async::Framed<tokio_core::net::TcpStream, websocket::async::MessageCodec<websocket::OwnedMessage>>;

#[async]
fn serve_client(client: UsualClient) -> std::io::Result<()> {
    let (mut sink, stream) = client.split();
    #[async]
    for m in stream.thaes() {
        if m.is_close() { break; }
        let fwd = match m {
            OwnedMessage::Ping(p) => OwnedMessage::Pong(p),
            OwnedMessage::Pong(_) => continue,
            _ => m,
        };
        sink = await!(sink.send(fwd).thaef())?;
    }
    await!(sink.send(OwnedMessage::Close(None)).thaef())?;
    Ok(())
}

fn main() {
	let mut core = Core::new().unwrap();
	let handle = core.handle();
	// bind to the server
	let server = Server::bind("127.0.0.1:9002", &handle).unwrap();

	// time to build the server's future
	// this will be a struct containing everything the server is going to do

	// a stream of incoming connections
	let str = server.incoming().thaes();
	
	let f = async_block! {
	    #[async]
	    for (upgrade, addr) in str {
            // accept the request to be a ws connection
            println!("Got a connection from: {}", addr);
            let (client, _headers) = await!(upgrade.accept().thaef())?;
            let f = serve_client(client);
            handle.spawn(f.map_err(|_|()).map(|_|()));
        }
        Ok::<(), std::io::Error>(())
    };

	core.run(f).unwrap();
}
