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
use std::cell::{RefCell,Cell};

use compactmap::CompactMap;
use std::collections::HashMap;

type UsualClient = websocket::client::async::Framed<tokio_core::net::TcpStream, websocket::async::MessageCodec<websocket::OwnedMessage>>;

type ClientSink = Rc<RefCell<futures::sync::mpsc::Sender<websocket::OwnedMessage>>>;
type AllClients = Rc<RefCell<CompactMap<ClientSink>>>;
type Url2Clientset = HashMap<String, AllClients>;

const BUFMSG : usize = 3;
const MAXURLS : usize = 64;

#[async]
fn serve_client(handle: Handle, client: UsualClient, all:AllClients) -> Result<(),()> {
    let (mut sink, stream) = client.split();
    let (snd,rcv) = futures::sync::mpsc::channel::<OwnedMessage>(BUFMSG);
    
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
        
        for (id, i) in all.borrow().iter() {
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
    
    let mapping : Rc<RefCell<Url2Clientset>> = Rc::new(RefCell::new(HashMap::new()));
    let num_urls : Rc<Cell<usize>> = Rc::new(Cell::new(0));
    
    let f = async_block! {
        #[async]
        for (upgrade, addr) in str {
            let url = format!("{}", upgrade.request.subject.1);
            println!("+ {} -> {}", addr, url);
            let (addr2, url2) = (addr.clone(), url.clone());
            
            let num_urls2 = num_urls.clone();
            let map_ = mapping.clone();
            let map2_ = mapping.clone();
            {
                let mut map = map_.borrow_mut();
                
                if !map.contains_key(&*url) {
                    if num_urls.get() >= MAXURLS {
                        println!("Rejected");
                        handle.spawn(upgrade.reject().map_err(|_|()).map(|_|()));
                        continue;
                    }
                
                    let new_clientset : AllClients = Rc::new(RefCell::new(CompactMap::new()));
                    map.insert(url.clone(), new_clientset);
                    println!("New URL: {}", url);
                    num_urls.set(num_urls.get() + 1);
                }
            }
            
            let (client, _headers) = await!(upgrade.accept().map_err(|_|()))?;
            
            let map = map_.borrow_mut();
            let clientset : AllClients = map.get(&*url).unwrap().clone();
            
            let f = serve_client(handle.clone(), client, clientset);
            
            handle.spawn(f.map_err(|_|()).map(move |_|{
                println!("- {} -> {}", addr2, url2);
                
                let mut map = map2_.borrow_mut();
                let mut do_remove = false;
                if let Some(x) = map.get(&*url2) {
                    if (*x).borrow().len_slow() == 0 {
                        println!("Expiring URL: {}", url2);
                        do_remove = true;
                    }
                }
                if do_remove {
                    map.remove(&*url2);
                    num_urls2.set(num_urls2.get() - 1);
                }
            }));
        }
        Ok::<(),()>(())
    };

    core.run(f).unwrap();
}
