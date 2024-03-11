use crate::store::ecg::{self, ECGHeader};
use async_session_types::{Eps, Recv, Send};
use bitvec::{order::Msb0, BitArr};
use std::num::TryFromIntError;

pub mod client;
pub mod server;
#[cfg(test)]
mod test;

/// TODO:
/// The session type for the ecg-sync protocol.
pub type ECGSync = Send<(), Eps>; // TODO

// Client:
//
// Send hashes of tips.
// Send hashes of (skipped) ancestors.
//
// Server:
//
// Send hashes of tips.
// Send hashes of (skipped) ancestors.
// Send bitmap indices of prev.have that we have.
//
// Loop until meet identified:
//
//   Client:
//
//   Send hashes of (skipped) ancestors.
//   Send bitmap indices of prev.have that we have.
//
//   Server:
//
//   Send hashes of (skipped) ancestors.
//   Send bitmap indices of prev.have that we have.
//
// Client:
//
// Send all headers he have that they don't (batched).
//
// Client:
//
// Send all headers he have that they don't (batched).
//

/// The maximum number of `have` hashes that can be sent in each message.
pub const MAX_HAVE_HEADERS: u16 = 32;
/// The maximum number of headers that can be sent in each message.
pub const MAX_DELIVER_HEADERS: u16 = 32;

#[derive(Debug, PartialEq)]
pub enum ECGSyncError {
    // We have too many tips to run the sync protocol.
    TooManyTips(TryFromIntError),
    // TODO: Timeout, IO error, connection terminated, etc...
}

pub enum MsgECGSync<H: ECGHeader> {
    Request(MsgECGSyncRequest<H>),
    Response(MsgECGSyncResponse<H>),
    Sync(MsgECGSyncData<H>),
}

#[derive(Debug)]
pub struct MsgECGSyncRequest<Header: ECGHeader> {
    /// Number of tips the client has.
    tip_count: u16,
    /// Hashes of headers the client has.
    /// The first `tip_count` hashes (potentially split across multiple messages) are tip headers.
    /// The maximum length is `MAX_HAVE_HEADERS`.
    have: Vec<Header::HeaderId>, // Should this include ancestors? Yes.
}

#[derive(Debug)]
pub struct MsgECGSyncResponse<Header: ECGHeader> {
    /// Number of tips the server has.
    tip_count: u16,
    /// `MsgECGSyncData` sync response.
    sync: MsgECGSyncData<Header>,
}

pub type HeaderBitmap = BitArr!(for MAX_HAVE_HEADERS as usize, in u8, Msb0);
#[derive(Debug)]
pub struct MsgECGSyncData<Header: ECGHeader> {
    /// Hashes of headers the server has.
    /// The first `tip_count` hashes (potentially split across multiple messages) are tip headers.
    /// The maximum length is `MAX_HAVE_HEADERS`.
    have: Vec<Header::HeaderId>,
    /// Bitmap of the hashes that the server knows from the previously sent headers `prev.have`.
    known: HeaderBitmap,
    /// Headers being delivered to the other party.
    /// The maximum length is `MAX_DELIVER_HEADERS`.
    headers: Vec<Header>,
}

// pub struct ECGSyncState<HeaderId> {
//     our_tips: Vec<HeaderId>,
// }
//
// impl<HeaderId> ECGSyncState<HeaderId> {
//     pub fn new(tips: Vec<HeaderId>) -> Self {
//         ECGSyncState {
//             our_tips: tips.to_vec(),
//         }
//     }
// }

use std::cmp::min;
use std::collections::{BTreeSet, BinaryHeap, VecDeque};
fn prepare_haves<Header: ECGHeader>(
    state: &ecg::State<Header>,
    queue: &mut BinaryHeap<(bool, u64, Header::HeaderId, u64)>,
    their_known: &BTreeSet<Header::HeaderId>,
    haves: &mut Vec<Header::HeaderId>,
) where
    Header::HeaderId: Copy + Ord,
{
    fn go<Header: ECGHeader>(
        state: &ecg::State<Header>,
        queue: &mut BinaryHeap<(bool, u64, Header::HeaderId, u64)>,
        their_known: &BTreeSet<Header::HeaderId>,
        haves: &mut Vec<Header::HeaderId>,
    ) where
        Header::HeaderId: Copy + Ord,
    {
        if haves.len() == MAX_HAVE_HEADERS.into() {
            return;
        }

        if let Some((_is_tip, depth, header_id, distance)) = queue.pop() {
            // If they already know this header, they already know its ancestors.
            let skip = their_known.contains(&header_id);
            if !skip {
                // If header is at an exponential distance (or is a child of the root node), send it with `haves`.
                if is_power_of_two(distance) || depth == 1 {
                    haves.push(header_id);
                }

                // Add parents to queue.
                if let Some(parents) = state.get_parents_with_depth(&header_id) {
                    for (depth, parent_id) in parents {
                        queue.push((false, depth, parent_id, distance + 1));
                    }
                } else {
                    // TODO XXX
                    todo!("Do we need to do anything?")
                }
            }

            go(state, queue, their_known, haves)
        }
    }

    haves.clear();
    go(state, queue, their_known, haves)
}

// Handle the haves that the peer sent to us.
// Returns the bitmap of which haves we know.
fn handle_received_have<Header: ECGHeader>(
    state: &ecg::State<Header>,
    their_tips_remaining: &mut usize,
    their_tips: &mut Vec<Header::HeaderId>,
    their_known: &mut BTreeSet<Header::HeaderId>,
    send_queue: &mut BinaryHeap<(u64, Header::HeaderId)>,
    have: &Vec<Header::HeaderId>,
    known_bitmap: &mut HeaderBitmap,
) where
    Header::HeaderId: Copy + Ord,
{
    // Accumulate their_tips.
    let provided_tip_c = min(*their_tips_remaining, have.len());
    their_tips.extend(&have[0..provided_tip_c]);
    *their_tips_remaining = *their_tips_remaining - provided_tip_c;
    // TODO: if their_tips is done, update peer_state.

    known_bitmap.fill(false);
    for (i, header_id) in have.iter().enumerate() {
        if state.contains(header_id) {
            // Record their known headers.
            mark_as_known(state, their_known, *header_id);

            // Respond with which headers we know.
            known_bitmap.set(i, true);

            // If we know the header, potentially send the children of that header.
            if let Some(children) = state.get_children_with_depth(&header_id) {
                send_queue.extend(children);
            } else {
                // TODO XXX
                todo!("Do we need to do anything?")
            }
        }
    }
}

// Handle (and verify) headers they sent to us.
// Returns if all the headers were valid.
fn handle_received_headers<Header: ECGHeader>(
    state: &mut ecg::State<Header>,
    headers: Vec<Header>,
) -> bool {
    let mut all_valid = true;
    for header in headers {
        // TODO: XXX
        // XXX
        // Verify header.
        // all_valid = all_valid && true;
        // XXX

        // Add to state.
        state.insert_header(header);
    }

    all_valid
}

// Precondition: `state` contains header_id.
// Invariant: if a header is in `their_known`, all the header's ancestors are in `their_known`.
fn mark_as_known<Header: ECGHeader>(
    state: &ecg::State<Header>,
    their_known: &mut BTreeSet<Header::HeaderId>,
    header_id: Header::HeaderId,
) where
    Header::HeaderId: Copy + Ord,
{
    fn go<Header: ECGHeader>(
        state: &ecg::State<Header>,
        their_known: &mut BTreeSet<Header::HeaderId>,
        mut queue: VecDeque<Header::HeaderId>,
    ) where
        Header::HeaderId: Copy + Ord,
    {
        if let Some(header_id) = queue.pop_front() {
            let contains = their_known.insert(header_id);
            if !contains {
                if let Some(parents) = state.get_parents(&header_id) {
                    queue.extend(parents);
                } else {
                    // TODO XXX
                    todo!("unreachable?")
                }
            }

            go(state, their_known, queue);
        }
    }

    let mut queue = VecDeque::new();
    queue.push_back(header_id);
    go(state, their_known, queue);
}

// Build the headers we will send to the peer.
fn prepare_headers<Header: ECGHeader>(
    state: &ecg::State<Header>,
    send_queue: &mut BinaryHeap<(u64, Header::HeaderId)>,
    their_known: &mut BTreeSet<Header::HeaderId>,
    headers: &mut Vec<Header>,
) where
    Header::HeaderId: Copy + Ord,
    Header: Clone,
{
    fn go<Header: ECGHeader>(
        state: &ecg::State<Header>,
        send_queue: &mut BinaryHeap<(u64, Header::HeaderId)>,
        their_known: &mut BTreeSet<Header::HeaderId>,
        headers: &mut Vec<Header>,
    ) where
        Header::HeaderId: Copy + Ord,
        Header: Clone,
    {
        if headers.len() == MAX_DELIVER_HEADERS.into() {
            return;
        }

        if let Some((_depth, header_id)) = send_queue.pop() {
            // Skip if they already know this header.
            let skip = their_known.contains(&header_id);
            if !skip {
                // Send header to peer.
                if let Some(header) = state.get_header(&header_id) {
                    headers.push(header.clone());

                    // Mark header as known by peer.
                    mark_as_known(state, their_known, header_id);
                } else {
                    // TODO XXX
                    todo!("unreachable?")
                }
            }

            // Add children to queue.
            if let Some(children) = state.get_children_with_depth(&header_id) {
                send_queue.extend(children);
            } else {
                // TODO XXX
                todo!("unreachable?")
            }

            go(state, send_queue, their_known, headers)
        }
    }

    headers.clear();
    go(state, send_queue, their_known, headers)
}

/// Check if the input is a power of two (inclusive of 0).
fn is_power_of_two(x: u64) -> bool {
    0 == (x & (x.wrapping_sub(1)))
}

fn handle_received_known<Header: ECGHeader>(
    state: &ecg::State<Header>,
    their_known: &mut BTreeSet<Header::HeaderId>,
    sent_haves: &Vec<Header::HeaderId>,
    received_known: &HeaderBitmap,
) where
    Header::HeaderId: Copy + Ord,
{
    for (i, header_id) in sent_haves.iter().enumerate() {
        // Check if they claimed they know this header.
        if *received_known
            .get(i)
            .expect("Unreachable since we're iterating of the headers we sent.")
        {
            // Mark header as known by them.
            mark_as_known(state, their_known, *header_id);
        }
    }
}

fn handle_received_ecg_sync<Header: ECGHeader>(
    sync_msg: MsgECGSyncData<Header>,
    state: &mut ecg::State<Header>,
    their_tips_remaining: &mut usize,
    their_tips: &mut Vec<Header::HeaderId>,
    their_known: &mut BTreeSet<Header::HeaderId>,
    send_queue: &mut BinaryHeap<(u64, Header::HeaderId)>,
    queue: &mut BinaryHeap<(bool, u64, Header::HeaderId, u64)>,
    haves: &mut Vec<Header::HeaderId>,
    headers: &mut Vec<Header>,
    known_bitmap: &mut HeaderBitmap,
) where
    Header::HeaderId: Copy + Ord,
    Header: Clone,
{
    // TODO: XXX
    // unimplemented!("Define ECGSyncState struct with all these variables");
    // XXX
    // XXX

    // Record which headers they say they already know.
    handle_received_known(state, their_known, haves, &sync_msg.known);

    // Receive (and verify) the headers they sent to us
    let all_valid = handle_received_headers(state, sync_msg.headers);
    // TODO: Record and exit if they sent invalid headers? Or tit for tat?

    // TODO: Check for no headers? their_tips_c == 0

    // Handle the haves that the peer sent to us.
    handle_received_have(
        state,
        their_tips_remaining,
        their_tips,
        their_known,
        send_queue,
        &sync_msg.have,
        known_bitmap,
    );

    // Send the headers we have.
    prepare_headers(state, send_queue, their_known, headers);

    // Propose headers we have.
    prepare_haves(state, queue, &their_known, haves);
}

trait ECGSyncMessage {
    /// Check if we're done based on this message.
    fn is_done(&self) -> bool;
}

impl<Header: ECGHeader> ECGSyncMessage for MsgECGSyncData<Header> {
    fn is_done(&self) -> bool {
        self.have.len() == 0 && self.headers.len() == 0
    }
}

impl<Header: ECGHeader> ECGSyncMessage for MsgECGSyncRequest<Header> {
    fn is_done(&self) -> bool {
        self.have.len() == 0
    }
}

impl<Header: ECGHeader> ECGSyncMessage for MsgECGSyncResponse<Header> {
    fn is_done(&self) -> bool {
        self.sync.is_done()
    }
}

impl<H: ECGHeader> Into<MsgECGSync<H>> for MsgECGSyncRequest<H> {
    fn into(self) -> MsgECGSync<H> {
        MsgECGSync::Request(self)
    }
}
impl<H: ECGHeader> Into<MsgECGSync<H>> for MsgECGSyncResponse<H> {
    fn into(self) -> MsgECGSync<H> {
        MsgECGSync::Response(self)
    }
}
impl<H: ECGHeader> Into<MsgECGSync<H>> for MsgECGSyncData<H> {
    fn into(self) -> MsgECGSync<H> {
        MsgECGSync::Sync(self)
    }
}
impl<H: ECGHeader> TryInto<MsgECGSyncRequest<H>> for MsgECGSync<H> {
    type Error = ();
    fn try_into(self) -> Result<MsgECGSyncRequest<H>, ()> {
        match self {
            MsgECGSync::Request(r) => Ok(r),
            MsgECGSync::Response(_) => Err(()),
            MsgECGSync::Sync(_) => Err(()),
        }
    }
}
impl<H: ECGHeader> TryInto<MsgECGSyncResponse<H>> for MsgECGSync<H> {
    type Error = ();
    fn try_into(self) -> Result<MsgECGSyncResponse<H>, ()> {
        match self {
            MsgECGSync::Request(_) => Err(()),
            MsgECGSync::Response(r) => Ok(r),
            MsgECGSync::Sync(_) => Err(()),
        }
    }
}
impl<H: ECGHeader> TryInto<MsgECGSyncData<H>> for MsgECGSync<H> {
    type Error = ();
    fn try_into(self) -> Result<MsgECGSyncData<H>, ()> {
        match self {
            MsgECGSync::Request(_) => Err(()),
            MsgECGSync::Response(_) => Err(()),
            MsgECGSync::Sync(s) => Ok(s),
        }
    }
}
