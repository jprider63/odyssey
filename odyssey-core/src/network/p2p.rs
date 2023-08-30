
/// Manage p2p network connections.

use log;
use std::fmt::Debug;
use std::marker::Send;
use std::net::{SocketAddrV4};
use std::thread;
use serde::Deserialize;
use tokio::net::{TcpListener,TcpStream};
use tokio_util::codec::{self, LengthDelimitedCodec};
use tokio_serde::formats;
use tokio_tower::multiplex;

use crate::protocol::Version;
use crate::network::protocol::{run_handshake_server, run_store_metadata_server};

pub struct P2PManager {
    p2p_thread: thread::JoinHandle<()>,

    // Store peer info and store metadata/raw data?
}

pub struct P2PSettings {
    pub address: SocketAddrV4,
}

impl P2PManager {
    pub fn initialize<StoreId>(settings: P2PSettings) -> P2PManager
    where
        StoreId:for<'a> Deserialize<'a> + Send + Debug,
    {
        // Spawn thread.
        let p2p_thread = thread::spawn(move || {
            // Start async runtime.
            let runtime = match tokio::runtime::Runtime::new() {
                Ok(r) => r,
                Err(err) => {
                    log::error!("Failed to initialize tokio runtime for P2P connections: {}", err);
                    return
                }
            };
            runtime.block_on(async {
                // Start listening for connections.
                let listener = match TcpListener::bind(&settings.address).await {
                    Ok(l) => l,
                    Err(err) => {
                        log::error!("Failed to bind to port ({}): {}", &settings.address, err);
                        return
                    }
                };

                loop {
                    // Accept connection.
                    let (tcpstream, peer) = match listener.accept().await {
                        Ok(r) => r,
                        Err(err) => {
                            log::error!("Failed to accept connection: {}", err);
                            continue
                        }
                    };
                    log::info!("Accepted connection from peer: {}", peer);

                    // Spawn async.
                    tokio::spawn(async {
                        // let (read_stream, write_stream) = tcpstream.split();
                        let mut stream = codec::Framed::new(tcpstream, LengthDelimitedCodec::new()); 

                        // TODO XXX
                        // Handshake.
                        // Diffie Hellman?
                        // Authenticate peer's public key?
                        let Version::V0 = run_handshake_server(&stream);

                        run_store_metadata_server::<StoreId, codec::Framed<TcpStream, LengthDelimitedCodec>>(&mut stream).await.expect("TODO");

                        // // Handle peer requests.
                        // let service = Echo;
                        // let stream = tokio_serde::Framed::new(stream, formats::Cbor::default());
                        // multiplex::Server::new(stream, service)
                    });
                }
            });
        });
    
        // Return handle to thread and channel.
        P2PManager {
            p2p_thread
        }
    }
}







// // TMP:
// use std::task::{Context, Poll};
// use std::pin::Pin;
// use std::future::Future;
// use serde::{Deserialize, Serialize};
// 
// /// A service that tokio-tower should serve over the transport.
// /// This one just echoes whatever it gets.
// struct Echo;
// 
// #[derive(Serialize, Deserialize, Debug)]
// struct MyMessage {
//     field: Vec<u8>,
// }
// 
// impl tower_service::Service<MyMessage> for Echo {
//     type Response = MyMessage; // T;
//     type Error = ();
//     // type Future = Ready<Result<Self::Response, Self::Error>>;
//     type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;
// 
//     fn poll_ready(&mut self, cx: &mut Context) -> Poll<Result<(), Self::Error>> {
//         Poll::Ready(Ok(()))
//     }
// 
//     fn call(&mut self, req: MyMessage) -> Self::Future {
//         println!("Received: {:?}", req);
//         // ready(Ok(req))
//         let fut = async {
//             Ok(req)
//             // Ok(vec![0,1,2,3,4,5,6,7])
//             // Ok(MyMessage{field: vec![0,1,2,3,4,5,6,7]})
//         };
//         Box::pin(fut)
//     }
// }
// 