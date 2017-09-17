#![feature(proc_macro, conservative_impl_trait, generators)]

extern crate websocket;
//extern crate futures;
extern crate futures_await as futures;
extern crate tokio_core;

use futures::prelude::*;

use websocket::message::OwnedMessage;
use websocket::server::InvalidConnection;
use websocket::async::Server;

use tokio_core::reactor::Core;
use futures::{Future, Sink, Stream};

fn main() {
	let mut core = Core::new().unwrap();
	let handle = core.handle();
	// bind to the server
	let server = Server::bind("127.0.0.1:9002", &handle).unwrap();

	// time to build the server's future
	// this will be a struct containing everything the server is going to do

	// a stream of incoming connections
	let str = server.incoming().map_err(
	   |InvalidConnection { error, .. }| 
	       std::io::Error::new(std::io::ErrorKind::ConnectionAborted, "...")
	);
	
	let f = async_block! {
	    #[async]
	    for x in str {
            let (upgrade, addr) = x;
            // accept the request to be a ws connection
            println!("Got a connection from: {}", addr);
            let f = upgrade
                .accept()
                .and_then(|(s, _)| {
                    // simple echo server impl
                    let (sink, stream) = s.split();
                    stream
                    .take_while(|m| Ok(!m.is_close()))
                    .filter_map(|m| {
                        match m {
                            OwnedMessage::Ping(p) => Some(OwnedMessage::Pong(p)),
                            OwnedMessage::Pong(_) => None,
                            _ => Some(m),
                        }
                    })
                    .forward(sink)
                    .and_then(|(_, sink)| {
                        sink.send(OwnedMessage::Close(None))
                    })
                });

	          handle.spawn(f.map_err(move |e| println!("{}: '{:?}'", addr, e))
	                       .map(move |_| println!("{} closed.", addr)));
        }
        Ok::<(), std::io::Error>(())
    };

	core.run(f).unwrap();
}
