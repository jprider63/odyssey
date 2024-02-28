
use crate::network::ConnectionManager;
use crate::network::protocol::ecg_sync::v0::client::ecg_sync_client;
use crate::network::protocol::ecg_sync::v0::server::ecg_sync_server;
use crate::store::ecg::{self, ECGHeader};
use crate::util::Channel;

#[derive(Clone)]
struct TestHeader {
    parent_ids: Vec<u32>,
}

// For testing, just have the header store the parent ids.
impl ECGHeader for TestHeader {
    type HeaderId = u32;

    fn get_parent_ids(self: &Self) -> &[u32] {
        &self.parent_ids
    }

    fn validate_header(&self, header_id: Self::HeaderId) -> bool {
        true
    }
}

fn run_ecg_sync<Header:ECGHeader + Clone>(st1: &mut ecg::State<Header>, st2: &mut ecg::State<Header>) {
    async fn future<Header:ECGHeader + Clone>(st1: &mut ecg::State<Header>, st2: &mut ecg::State<Header>) {
        let store_id = 0_u64;
        // let channel: Channel<bytes::Bytes> = Channel::new();
        // let channel: Channel<Result<BytesMut, std::io::Error>> = Channel::new();
        let channel: Channel<Result<bytes::Bytes, std::io::Error>> = Channel::new();
        let conn = ConnectionManager::new(channel);

        let server = ecg_sync_server(&conn, &store_id, st1);
        let client = ecg_sync_client(&conn, &store_id, st1);

        let (server_res, client_res) = tokio::join!(server, client);

        assert_eq!(server_res, Ok(()));
        assert_eq!(client_res, Ok(()));
    }

    let mut rt = tokio::runtime::Runtime::new().expect("Failed to start tokio runtime");
    rt.block_on(future(st1, st2));
}

fn add_ops(st: &mut ecg::State<TestHeader>, ops: &[(u32,&[u32])]) {
    for (header_id, parent_ids) in ops {
        let header = TestHeader {
            parent_ids: parent_ids.to_vec(),
        };
        assert!(st.insert_header(*header_id, header), "Failed to insert header");
    }
}

fn test_helper(common: &[(u32,&[u32])], left: &[(u32,&[u32])], right: &[(u32,&[u32])]) {
    let mut left_tree = ecg::State::new();
    add_ops(&mut left_tree, common);

    let mut right_tree = left_tree.clone();
    
    add_ops(&mut left_tree, left);
    add_ops(&mut right_tree, right);

    run_ecg_sync(&mut left_tree, &mut right_tree);

    assert!(ecg::equal_dags(&left_tree, &right_tree));
}

fn test_both(common: &[(u32,&[u32])], left: &[(u32,&[u32])], right: &[(u32,&[u32])]) {
    test_helper(common, left, right);
    test_helper(common, right, left);
}

#[test]
fn empty1() {
    test_both(&[], &[], &[]);
}

#[test]
fn empty2() {
    test_both(&[], &[
              (0,&[]),
              (1,&[0]),
        ],
        &[
              (2,&[]),
              (3,&[]),
              (4,&[2,3]),
        ],
    );
}

#[test]
fn empty_one1() {
    test_both(&[], &[], &[
              (0,&[]),
              (1,&[0]),
    ]);
}

#[test]
fn empty_one2() {
    test_both(
        &[
              (0,&[]),
              (1,&[0]),
              (2,&[]),
        ],
        &[
              (3,&[]),
        ],
        &[
        ],
    );
}

#[test]
fn empty_one3() {
    test_both(
        &[
              (0,&[]),
              (1,&[0]),
              (2,&[]),
        ],
        &[
              (3,&[1,2]),
        ],
        &[
        ],
    );
}

#[test]
fn concurrent() {
    test_both(
        &[
              (0,&[]),
              (1,&[0]),
              (2,&[]),
        ],
        &[
              (3,&[1,2]),
        ],
        &[
              (4,&[1]),
        ],
    );
}

#[test]
fn cross() {
    test_both(
        &[
              (0,&[]),
              (1,&[0]),
              (2,&[]),
        ],
        &[
              (3,&[1,2]),
        ],
        &[
              (4,&[0,2]),
        ],
    );
}
