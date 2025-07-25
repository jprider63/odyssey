use odyssey_crdt::time::CausalState;
// use futures::{SinkExt, StreamExt};
// use futures_channel::mpsc::{UnboundedReceiver, UnboundedSender};
use odyssey_crdt::CRDT;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Debug, Display};
use std::marker::PhantomData;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Arc;
use std::thread;
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Runtime;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::{watch, RwLock};
use tokio::task::JoinHandle;
use tokio_util::codec::{self, LengthDelimitedCodec};
use tracing::{debug, error, info, warn};
use typeable::Typeable;

use crate::auth::{generate_identity, DeviceId, Identity};
use crate::network::protocol::{run_handshake_client, run_handshake_server, HandshakeError};
use crate::protocol::manager::v0::PeerManagerCommand;
use crate::protocol::MiniProtocolArgs;
use crate::storage::Storage;
use crate::store::ecg::{self, ECGBody, ECGHeader};
use crate::store::{self, StateUpdate, StoreCommand, UntypedStoreCommand};
use crate::time::ConcretizeTime;
use crate::util::{self, TypedStream};

pub struct Odyssey<OT: OdysseyType> {
    /// Thread running the Odyssey server.
    thread: thread::JoinHandle<()>,
    // command_channel: UnboundedSender<OdysseyCommand>,
    tokio_runtime: Runtime,
    /// Active stores.
    // stores: BTreeMap<OT::StoreId,ActiveStore>,
    active_stores: watch::Sender<
        StoreStatuses<OT::StoreId, OT::Hash, <OT::ECGHeader as ECGHeader>::HeaderId, OT::ECGHeader>,
    >, // JP: Make this encode more state that other's may want to subscribe to?
    shared_state: SharedState<OT::StoreId>, // JP: Could have another thread own and manage this state
    // instead?
    phantom: PhantomData<OT>,
    identity_keys: Identity,
}
pub type StoreStatuses<StoreId, Hash, HeaderId, Header> =
    BTreeMap<StoreId, StoreStatus<Hash, HeaderId, Header>>; // Rename this MiniProtocolArgs?

// pub enum StoreStatus<O: OdysseyType, T: CRDT<Time = O::Time>>
// where
//     T::Op: Serialize,
pub enum StoreStatus<Hash, HeaderId, Header> {
    // Store is initializing and async handler is being created.
    Initializing,
    // Store's async handler is running.
    Running {
        store_handle: JoinHandle<()>, // JP: Does this belong here? The state is owned here, but
        // the miniprotocols probably don't need to block waiting on
        // it...
        // send_command_chan: UnboundedSender<StoreCommand<O::ECGHeader, T>>,
        // https://www.reddit.com/r/rust/comments/1exjiab/the_amazing_pattern_i_discovered_hashmap_with/
        // send_command_chan: UnboundedSender<StoreCommand<store::ecg::v0::Header<dyn Hash, dyn CRDT>, dyn CRDT>>,
        // send_command_chan: UnboundedSender<UntypedStoreCommand>,
        send_command_chan: UnboundedSender<UntypedStoreCommand<Hash, HeaderId, Header>>,
    },
}

#[derive(Clone, Debug)]
/// Odyssey state that is shared across multiple tasks.
pub(crate) struct SharedState<StoreId> {
    pub(crate) peer_state:
        Arc<RwLock<BTreeMap<DeviceId, UnboundedSender<PeerManagerCommand<StoreId>>>>>,
}

impl<Hash, HeaderId, Header> StoreStatus<Hash, HeaderId, Header> {
    pub(crate) fn is_initializing(&self) -> bool {
        match self {
            StoreStatus::Initializing => true,
            StoreStatus::Running { .. } => false,
        }
    }

    pub(crate) fn is_initialized(&self) -> bool {
        !self.is_initializing()
    }

    pub(crate) fn command_channel(
        &self,
    ) -> Option<&UnboundedSender<UntypedStoreCommand<Hash, HeaderId, Header>>> {
        match self {
            StoreStatus::Initializing => None,
            StoreStatus::Running {
                send_command_chan, ..
            } => Some(send_command_chan),
        }
    }
}

impl<OT: OdysseyType> Odyssey<OT> {
    async fn bind_server_ipv4(mut port: u16) -> Option<TcpListener> {
        for _ in 0..10 {
            let address = SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), port);
            match TcpListener::bind(&address).await {
                Ok(l) => {
                    info!("Started server: {address}");
                    return Some(l);
                }
                Err(err) => {
                    warn!("Failed to bind to port ({}): {}", &address, err);
                    port += 1;
                }
            }
        }

        None
    }

    // Start odyssey.
    pub fn start(config: OdysseyConfig) -> Self {
        // TODO: Load identity or take it as an argument.
        let identity_keys = generate_identity();

        // // Create channels to communicate with Odyssey thread.
        // let (send_odyssey_commands, mut recv_odyssey_commands) = futures_channel::mpsc::unbounded();
        let (active_stores, active_stores_receiver) = watch::channel(BTreeMap::new());
        let device_id = DeviceId::new(identity_keys.auth_key().verifying_key());

        let shared_state_ = SharedState {
            peer_state: Arc::new(RwLock::new(BTreeMap::new())),
        };

        // Start async runtime.
        let runtime = match tokio::runtime::Runtime::new() {
            Ok(r) => r,
            Err(err) => {
                error!("Failed to initialize tokio runtime: {}", err);
                todo!()
            }
        };
        let runtime_handle = runtime.handle().clone();
        let shared_state = shared_state_.clone();

        // Spawn server thread.
        let odyssey_thread = thread::spawn(move || {
            runtime_handle.block_on(async move {
                // Start listening for connections.
                let Some(listener) = Odyssey::<OT>::bind_server_ipv4(config.port).await else {
                    error!("Failed to start server.");
                    return;
                };

                // // Handle commands from application.
                // tokio::spawn(async move {
                //     while let Some(cmd) = recv_odyssey_commands.next().await {
                //         todo!();
                //     }

                //     unreachable!();
                // });

                info!("Starting server");
                loop {
                    // Accept connection.
                    let (tcpstream, peer) = match listener.accept().await {
                        Ok(r) => r,
                        Err(err) => {
                            error!("Failed to accept connection: {}", err);
                            continue;
                        }
                    };
                    info!("Accepted connection from peer: {}", peer);
                    // Spawn async.
                    let active_stores = active_stores_receiver.clone();
                    // let device_id = DeviceId::new(identity_keys.auth_key().verifying_key());
                    let shared_state = shared_state.clone();

                    let future_handle = tokio::spawn(async move {
                        // let (read_stream, write_stream) = tcpstream.split();
                        let stream = codec::Framed::new(tcpstream, LengthDelimitedCodec::new());

                        // TODO XXX
                        // Handshake.
                        // Diffie Hellman? TLS?
                        // Authenticate peer's public key?
                        let mut stream = TypedStream::new(stream);
                        let handshake_result = run_handshake_server(&mut stream, &device_id).await;
                        let stream = stream.finalize().into_inner();

                        let handshake_result = match handshake_result {
                            Ok(r) => r,
                            Err(HandshakeError::ConnectingToSelf) => {
                                info!("Disconnecting. Attempting to connect to ourself.");
                                return;
                            }
                        };

                        info!(
                            "Handshake complete with peer: {}",
                            handshake_result.peer_id()
                        );
                        // Store peer in state.
                        if let Some(recv) =
                            initiate_peer(handshake_result.peer_id(), &shared_state).await
                        {
                            // Start miniprotocols.
                            let args = MiniProtocolArgs::new(
                                handshake_result.peer_id(),
                                active_stores,
                                recv,
                            );
                            handshake_result
                                .version()
                                .run_miniprotocols_server::<OT>(stream, args)
                                .await;
                        } else {
                            info!(
                                "Disconnecting. Already connected to peer: {}",
                                handshake_result.peer_id()
                            );
                        }
                    });
                }
            });
        });

        // TODO: Store identity key

        Odyssey {
            thread: odyssey_thread,
            // command_channel: send_odyssey_commands,
            tokio_runtime: runtime,
            active_stores,
            phantom: PhantomData,
            shared_state: shared_state_,
            identity_keys,
        }
    }

    pub fn create_store<T, S: Storage>(&self, initial_state: T, _storage: S) -> StoreHandle<OT, T>
    where
        T: CRDT<Time = OT::Time>
            + Clone
            + Debug
            + Send
            + 'static
            + Typeable
            + Serialize
            + for<'d> Deserialize<'d>,
        // T::Op<CausalTime<OT::Time>>: Serialize,
        // T::Op: ConcretizeTime<T::Time>, // <OT::ECGHeader as ECGHeader>::HeaderId>,
        T::Op: ConcretizeTime<<OT::ECGHeader as ECGHeader>::HeaderId>,
        OT::ECGBody<T>: Send
            + Serialize
            + for<'d> Deserialize<'d>
            + Debug
            + ECGBody<
                T::Op,
                <T::Op as ConcretizeTime<<OT::ECGHeader as ECGHeader>::HeaderId>>::Serialized,
                Header = OT::ECGHeader,
            >,
        OT::ECGHeader: Send + Sync + Clone + 'static + Serialize + for<'d> Deserialize<'d>,
        // OT::ECGBody<T>:
        //     Send + ECGBody<T, Header = OT::ECGHeader> + Serialize + for<'d> Deserialize<'d> + Debug,
        <OT::ECGHeader as ECGHeader>::HeaderId: Send + Serialize + for<'d> Deserialize<'d>,
    {
        // Create store by generating nonce, etc.
        let store = store::State::<OT::StoreId, OT::ECGHeader, T, OT::Hash>::new_syncing(
            initial_state.clone(),
        );
        let store_id = store.store_id();

        // Check if this store id already exists and try again if there's a conflict.
        // Otherwise, mark this store as initializing.
        let mut already_exists = false;
        self.active_stores.send_if_modified(|active_stores| {
            let res = active_stores.try_insert(store_id, StoreStatus::Initializing);
            if res.is_err() {
                already_exists = true;
            }
            false
        });
        if already_exists {
            // This will generate a new nonce if there's a conflict.
            return self.create_store(initial_state, _storage);
        }

        // Launch the store.
        let store_handle = self.launch_store(store_id, store);
        info!("Created store: {}", store_id);
        store_handle
    }

    pub fn connect_to_store<T>(
        &self,
        store_id: OT::StoreId,
        // storage: S,
    ) -> StoreHandle<OT, T>
    where
        OT::ECGHeader: Send + Sync + Clone + 'static,
        T::Op: ConcretizeTime<<OT::ECGHeader as ECGHeader>::HeaderId>,
        OT::ECGBody<T>: Send
            + Serialize
            + for<'d> Deserialize<'d>
            + Debug
            + ECGBody<
                T::Op,
                <T::Op as ConcretizeTime<<OT::ECGHeader as ECGHeader>::HeaderId>>::Serialized,
                Header = OT::ECGHeader,
            >,
        // T::Op: ConcretizeTime<T::Time>,
        // OT::ECGBody<T>:
        //     Send + ECGBody<T, Header = OT::ECGHeader> + Serialize + for<'d> Deserialize<'d> + Debug,
        <<OT as OdysseyType>::ECGHeader as ECGHeader>::HeaderId: Send,
        T: CRDT<Time = OT::Time> + Clone + Debug + Send + 'static + for<'d> Deserialize<'d>,
        // T::Op<CausalTime<OT::Time>>: Serialize,
    {
        // Check if store is already active.
        // If it isn't, mark it as initializing and continue.
        let mut is_active = false;
        self.active_stores.send_if_modified(|active_stores| {
            let res = active_stores.try_insert(store_id, StoreStatus::Initializing);
            if res.is_err() {
                is_active = true;
            }
            false
        });
        if is_active {
            // TODO: Get handle of existing store.
            return todo!();
        }

        // TODO:
        // - Load store from disk if we have it locally.
        // Spawn async handler.
        let state = store::State::new_downloading(store_id);
        let store_handler = self.launch_store(store_id, state);
        debug!("Joined store: {}", store_id);
        store_handler

        // - Add it to our active store set with the appropriate status.
        //
        // Update our peers + sync with them? This is automatic?
        //
        // TODO: Set status as initializing in create_store too
    }

    // Connect to network.
    pub fn connect() {
        todo!("Turn on network connection")
    }

    // Disconnect from network.
    pub fn disconnect() {
        todo!("Turn off network connections (work offline)")
    }

    fn device_id(&self) -> DeviceId {
        DeviceId::new(self.identity_keys.auth_key().verifying_key())
    }

    // Connect to a peer over ipv4.
    pub fn connect_to_peer_ipv4(&self, address: SocketAddrV4) {
        let active_stores = self.active_stores.subscribe();
        let device_id = self.device_id();
        let shared_state = self.shared_state.clone();

        // Spawn async.
        let future_handle = self.tokio_runtime.spawn(async move {
            // Attempt to connect to peer, returning message on failure.
            let mut stream = match TcpStream::connect(address).await {
                Ok(tcpstream) => {
                    let stream = codec::Framed::new(tcpstream, LengthDelimitedCodec::new());
                    TypedStream::new(stream)
                }
                Err(err) => {
                    todo!("TODO: Log error");
                    return;
                }
            };

            // Run client handshake.
            let handshake_result = run_handshake_client(&mut stream, &device_id).await;
            let stream = stream.finalize().into_inner();
            debug!("Connected to server!");

            let handshake_result = match handshake_result {
                Ok(r) => r,
                Err(HandshakeError::ConnectingToSelf) => {
                    info!("Disconnecting. Attempting to connect to ourself.");
                    return;
                }
            };

            info!(
                "Handshake complete with peer: {}",
                handshake_result.peer_id()
            );
            // Store peer in state.
            if let Some(recv) = initiate_peer(handshake_result.peer_id(), &shared_state).await {
                // Start miniprotocols.
                debug!("Start miniprotocols");
                let args = MiniProtocolArgs::new(handshake_result.peer_id(), active_stores, recv);
                handshake_result
                    .version()
                    .run_miniprotocols_client::<OT>(stream, args)
                    .await;
            } else {
                info!(
                    "Disconnecting. Already connected to peer: {}",
                    handshake_result.peer_id()
                );
            }
        });

        // Return channel with peer connection status.
    }

    // TODO: Separate state (that keeps state, syncs with other peers, etc) and optional user API (that sends state updates)?
    fn launch_store<T>(
        &self,
        store_id: OT::StoreId,
        store: store::State<OT::StoreId, OT::ECGHeader, T, OT::Hash>,
    ) -> StoreHandle<OT, T>
    where
        OT::ECGHeader: Send + Sync + Clone + 'static + for<'d> Deserialize<'d> + Serialize,
        T::Op: ConcretizeTime<<OT::ECGHeader as ECGHeader>::HeaderId>,
        OT::ECGBody<T>: Send
            + Serialize
            + for<'d> Deserialize<'d>
            + Debug
            + ECGBody<
                T::Op,
                <T::Op as ConcretizeTime<<OT::ECGHeader as ECGHeader>::HeaderId>>::Serialized,
                Header = OT::ECGHeader,
            >,
        // OT::ECGBody<T>:
        //     Send + ECGBody<T, Header = OT::ECGHeader> + Serialize + for<'d> Deserialize<'d> + Debug,
        <<OT as OdysseyType>::ECGHeader as ECGHeader>::HeaderId:
            Send + for<'d> Deserialize<'d> + Serialize,
        // T::Op<CausalTime<OT::Time>>: Serialize,
        T: CRDT<Time = OT::Time> + Debug + Clone + Send + 'static + for<'d> Deserialize<'d>,
    {
        // Initialize storage for this store.

        // Create channels to handle requests and send updates.
        let (send_commands, recv_commands) = tokio::sync::mpsc::unbounded_channel::<
            store::StoreCommand<OT::ECGHeader, OT::ECGBody<T>, T>,
        >();
        let (send_commands_untyped, recv_commands_untyped) = tokio::sync::mpsc::unbounded_channel::<
            store::UntypedStoreCommand<
                OT::Hash,
                <OT::ECGHeader as ECGHeader>::HeaderId,
                OT::ECGHeader,
            >,
        >();

        // Add to DHT

        // Spawn routine that owns this store.

        let shared_state = self.shared_state.clone();
        let send_commands_untyped_ = send_commands_untyped.clone();
        let future_handle = self.tokio_runtime.spawn(async move {
            store::run_handler::<OT, T>(
                store,
                recv_commands,
                send_commands_untyped_,
                recv_commands_untyped,
                shared_state,
            )
            .await;
        });

        // Register this store.
        self.active_stores.send_if_modified(|active_stores| {
            let _ = active_stores.insert(
                store_id,
                StoreStatus::Running {
                    store_handle: future_handle,
                    send_command_chan: send_commands_untyped,
                },
            );
            true
        });

        StoreHandle {
            // future_handle,
            send_command_chan: send_commands,
            phantom: PhantomData,
        }
    }
}

/// Initiates a peer by creating a channel to send commands and by inserting it into the shared state. On success, returns the receiver. If the peer already exists, fails with `None`.
async fn initiate_peer<StoreId>(
    peer_id: DeviceId,
    shared_state: &SharedState<StoreId>,
) -> Option<UnboundedReceiver<PeerManagerCommand<StoreId>>> {
    let (send, recv) = tokio::sync::mpsc::unbounded_channel();
    let inserted = {
        let mut w = shared_state.peer_state.write().await;
        w.try_insert(peer_id, send).is_ok()
    };
    if inserted {
        Some(recv)
    } else {
        // JP: Record if we're already connected to the peer?
        None
    }
}

#[derive(Clone, Copy)]
pub struct OdysseyConfig {
    // IPv4 port to run Odyssey on.
    pub port: u16,
}

pub struct StoreHandle<
    O: OdysseyType,
    T: CRDT<Time = O::Time, Op: ConcretizeTime<<O::ECGHeader as ECGHeader>::HeaderId>>,
>
// where
//     // T::Op: Serialize,
//     T::Op<CausalTime<OT::Time>>: Serialize,
{
    // future_handle: JoinHandle<()>, // JP: Maybe this should be owned by `Odyssey`?
    send_command_chan: UnboundedSender<StoreCommand<O::ECGHeader, O::ECGBody<T>, T>>,
    phantom: PhantomData<O>,
}

/// Trait to define newtype wrapers that instantiate type families required by Odyssey.
pub trait OdysseyType: 'static {
    type StoreId: Debug
        + Display
        + Eq
        + Copy
        + Ord
        + Send
        + Sync
        + 'static
        + Serialize
        + for<'a> Deserialize<'a>
        + AsRef<[u8]>; // Hashable instead of AsRef???
    type Hash: util::Hash
        + Debug
        + Display
        + Copy
        + Ord
        + Send
        + Sync
        + 'static
        + Serialize
        + for<'a> Deserialize<'a>
        + Into<Self::StoreId>; // Hashable instead of AsRef???
                               // type ECGHeader<T: CRDT<Time = Self::Time, Op: Serialize>>: store::ecg::ECGHeader + Debug + Send;
    type ECGHeader: store::ecg::ECGHeader<HeaderId: Send + Sync + Serialize + for<'a> Deserialize<'a>>
        + Debug
        + Send
        + Serialize
        + for<'a> Deserialize<'a>;
    type ECGBody<T: CRDT<Op: ConcretizeTime<<Self::ECGHeader as ECGHeader>::HeaderId>>>; // : Serialize + for<'a> Deserialize<'a>; // : CRDT<Time = Self::Time, Op: Serialize>;
    type Time;
    // type CausalState<T: CRDT<Time = Self::Time, Op<CausalTime<Self::Time>>: Serialize>>: CausalState<Time = Self::Time>;
    type CausalState<T: CRDT<Time = Self::Time>>: CausalState<Time = Self::Time>;
    // type OperationId;
    // type Hash: Clone + Copy + Debug + Ord + Send;

    // TODO: This should be refactored and provided automatically.
    fn to_causal_state<T: CRDT<Time = Self::Time>>(
        st: &store::ecg::State<Self::ECGHeader, T>,
    ) -> &Self::CausalState<T>;
}

impl<
        O: OdysseyType,
        T: CRDT<Time = O::Time, Op: ConcretizeTime<<O::ECGHeader as ECGHeader>::HeaderId>>,
    > StoreHandle<O, T>
// where
//     T::Op<CausalTime<T::Time>>: Serialize,
{
    pub fn apply(
        &mut self,
        parents: BTreeSet<<O::ECGHeader as ECGHeader>::HeaderId>,
        op: <T::Op as ConcretizeTime<<O::ECGHeader as ECGHeader>::HeaderId>>::Serialized,
    ) -> <O::ECGHeader as ECGHeader>::HeaderId
    where
        T::Op: ConcretizeTime<<O::ECGHeader as ECGHeader>::HeaderId>,
        O::ECGBody<T>: ECGBody<
            T::Op,
            <T::Op as ConcretizeTime<<O::ECGHeader as ECGHeader>::HeaderId>>::Serialized,
            Header = O::ECGHeader,
        >,
    {
        self.apply_batch(parents, vec![op])
    }

    // TODO: Don't take parents as an argument. Pull it from the state. XXX
    pub fn apply_batch(
        &mut self,
        parents: BTreeSet<<<O as OdysseyType>::ECGHeader as ECGHeader>::HeaderId>,
        op: Vec<<T::Op as ConcretizeTime<<O::ECGHeader as ECGHeader>::HeaderId>>::Serialized>, // T::Op<CausalTime<T::Time>>>,
                                                                                               // op: Vec<T::Op>,
    ) -> <O::ECGHeader as ECGHeader>::HeaderId
    where
        T::Op: ConcretizeTime<<O::ECGHeader as ECGHeader>::HeaderId>,
        <O as OdysseyType>::ECGBody<T>: ECGBody<
            T::Op,
            <T::Op as ConcretizeTime<<O::ECGHeader as ECGHeader>::HeaderId>>::Serialized,
            Header = O::ECGHeader,
        >,
    {
        // TODO: Divide into 256 operation chunks.
        // if op.is_empty() {
        //     return vec![];
        // }

        // Create ECG header and body.
        let body = <<O as OdysseyType>::ECGBody<T> as ECGBody<
            T::Op,
            <T::Op as ConcretizeTime<<O::ECGHeader as ECGHeader>::HeaderId>>::Serialized,
        >>::new_body(op);
        let header = body.new_header(parents);
        let header_id = header.get_header_id();
        // let times = body.get_operation_times(&header);

        self.send_command_chan
            .send(StoreCommand::Apply {
                operation_header: header,
                operation_body: body,
            })
            .expect("TODO");

        // times
        header_id
    }

    pub fn subscribe_to_state(&mut self) -> UnboundedReceiver<StateUpdate<O::ECGHeader, T>> {
        let (send_state, recv_state) = tokio::sync::mpsc::unbounded_channel();
        self.send_command_chan
            .send(StoreCommand::SubscribeState { send_state })
            .expect("TODO");

        recv_state
    }
}

// pub enum OdysseyCommand {
//     CreateStore {
//         // Since Rust doesn't have existentials...
//         initial_state: (), // Box<Dynamic>, // T
//         storage: Box<dyn Storage + Send>,
//     },
// }

// fn handle_odyssey_command() {
// }
