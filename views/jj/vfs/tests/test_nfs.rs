//! Drives the NFSv3 server end to end over a loopback socket with an
//! in-process Rust NFS client.
//!
//! This is the cheapest test that exercises real XDR encoding, the RPC framing,
//! the MOUNT handshake and every NFS procedure we implement. It needs no
//! privileges and no kernel mount, so it runs on any platform including in a
//! plain build sandbox. The NixOS VM test covers the part this cannot: the
//! operating system's own NFS client talking to us.

use std::sync::Arc;

use jj_lib::backend::MillisSinceEpoch;
use jj_lib::backend::Timestamp;
use jj_lib::repo::Repo as _;
use jj_vfs::TreeSnapshot;
use jj_vfs::default_materialize_options;
use jj_vfs::nfs::NfsTree;
use nfs3_client::Nfs3ConnectionBuilder;
use nfs3_client::nfs3_types::nfs3::GETATTR3args;
use nfs3_client::nfs3_types::nfs3::LOOKUP3args;
use nfs3_client::nfs3_types::nfs3::Nfs3Option;
use nfs3_client::nfs3_types::nfs3::READ3args;
use nfs3_client::nfs3_types::nfs3::READDIRPLUS3args;
use nfs3_client::nfs3_types::nfs3::READLINK3args;
use nfs3_client::nfs3_types::nfs3::diropargs3;
use nfs3_client::nfs3_types::nfs3::ftype3;
use nfs3_client::nfs3_types::nfs3::nfs_fh3;
use nfs3_client::nfs3_types::nfs3::nfsstat3;
use nfs3_client::tokio::TokioConnector;
use nfs3_client::tokio::TokioIo;
use nfs3_server::tcp::NFSTcp as _;
use nfs3_server::tcp::NFSTcpListener;
use pretty_assertions::assert_eq;
use testutils::TestRepo;
use testutils::TestThreeWayMergeTreeBuilder;
use testutils::TestTreeBuilder;
use testutils::repo_path;
use tokio::net::TcpStream;

/// What `TokioConnector` hands back. Spelled out because naming the `impl`
/// bound inline does not fit the line limit.
type TestConnection = nfs3_client::Nfs3Connection<TokioIo<TcpStream>>;

const TEST_TIME: Timestamp = Timestamp {
    timestamp: MillisSinceEpoch(1_769_000_000_000),
    tz_offset: 0,
};

/// Serves `snapshot` on an ephemeral loopback port and returns a mounted
/// client. The server task is left running; it stops when the test process
/// exits, which is what a `#[tokio::test]` runtime does at the end of the test.
async fn serve(snapshot: TreeSnapshot) -> TestConnection {
    let tree = NfsTree::new(Arc::new(snapshot), 0x6a6a_0001, 501, 20);
    // Port 0 lets the OS pick, so concurrent tests cannot collide on a port.
    let listener = NFSTcpListener::bind_ro("127.0.0.1:0", tree)
        .await
        .expect("bind a loopback NFS listener");
    let port = listener.get_listen_port();
    tokio::spawn(async move {
        // handle_forever only returns on a listener error, which ends the test
        // by way of the client failing rather than silently.
        drop(listener.handle_forever().await);
    });
    Nfs3ConnectionBuilder::new(TokioConnector, "127.0.0.1", "/")
        // A reserved source port needs root. Nothing about the protocol
        // requires one, and requiring it would mean `jj fs mount` needed sudo.
        .connect_from_privileged_port(false)
        // Both ports are given explicitly so no portmapper (port 111) is
        // needed. There is no rpcbind in a build sandbox, and on macOS talking
        // to the system one is what breaks other loopback NFS servers.
        .mount_port(port)
        .nfs3_port(port)
        .mount()
        .await
        .expect("MOUNT and NFS handshake against our own server")
}

fn snapshot_of(tree: &jj_lib::merged_tree::MergedTree) -> TreeSnapshot {
    let options = default_materialize_options(tree.store().merge_options().clone());
    pollster::block_on(TreeSnapshot::new(tree, options, &TEST_TIME, 1 << 20)).expect("snapshot")
}

fn name(value: &str) -> nfs3_client::nfs3_types::nfs3::filename3<'static> {
    nfs3_client::nfs3_types::nfs3::filename3::from(value.as_bytes().to_vec())
}

#[tokio::test]
async fn test_nfs_readdirplus_lookup_read_and_readlink() {
    let test_repo = TestRepo::init();
    let mut builder = TestTreeBuilder::new(test_repo.repo.store().clone());
    builder.file(repo_path("hello.txt"), "hello nfs\n");
    builder
        .file(repo_path("run.sh"), "#!/bin/sh\ntrue\n")
        .executable(true);
    builder.symlink(repo_path("link"), "hello.txt");
    builder.file(repo_path("sub/inner.txt"), "inner\n");
    let tree = builder.write_merged_tree();

    let mut connection = serve(snapshot_of(&tree)).await;
    let root = connection.root_nfs_fh3();
    let client = &mut connection.nfs3_client;

    // READDIRPLUS over the root.
    let listing = client
        .readdirplus(&READDIRPLUS3args {
            dir: root.clone(),
            cookie: 0,
            cookieverf: Default::default(),
            dircount: 4096,
            maxcount: 65536,
        })
        .await
        .expect("readdirplus rpc")
        .unwrap();
    let names: Vec<String> = listing
        .reply
        .entries
        .0
        .iter()
        .map(|entry| String::from_utf8(entry.name.0.as_ref().to_vec()).expect("utf-8 name"))
        .collect();
    assert_eq!(names, ["hello.txt", "link", "run.sh", "sub"]);
    assert!(listing.reply.eof, "the whole listing fits in one reply");

    // LOOKUP then GETATTR then READ, which is exactly what a kernel client does
    // to `cat` a file.
    let looked_up = client
        .lookup(&LOOKUP3args {
            what: diropargs3 {
                dir: root.clone(),
                name: name("hello.txt"),
            },
        })
        .await
        .expect("lookup rpc")
        .unwrap();
    let file = looked_up.object;
    let attributes = client
        .getattr(&GETATTR3args {
            object: file.clone(),
        })
        .await
        .expect("getattr rpc")
        .unwrap()
        .obj_attributes;
    assert_eq!(attributes.type_, ftype3::NF3REG);
    assert_eq!(attributes.size, 10);
    assert_eq!(
        attributes.mode, 0o444,
        "a read-only mount must not offer a write bit"
    );
    let read = client
        .read(&READ3args {
            file,
            offset: 0,
            count: 4096,
        })
        .await
        .expect("read rpc")
        .unwrap();
    assert_eq!(read.data.as_ref(), b"hello nfs\n");
    assert!(read.eof);

    // The executable bit has to survive the whole round trip, or a mounted
    // build tree cannot run its own scripts.
    let script = client
        .lookup(&LOOKUP3args {
            what: diropargs3 {
                dir: root.clone(),
                name: name("run.sh"),
            },
        })
        .await
        .expect("lookup rpc")
        .unwrap()
        .object;
    let script_attributes = client
        .getattr(&GETATTR3args { object: script })
        .await
        .expect("getattr rpc")
        .unwrap()
        .obj_attributes;
    assert_eq!(script_attributes.mode, 0o555);

    // READLINK.
    let link = client
        .lookup(&LOOKUP3args {
            what: diropargs3 {
                dir: root.clone(),
                name: name("link"),
            },
        })
        .await
        .expect("lookup rpc")
        .unwrap();
    // LOOKUP carries the target's attributes, so a client learns it is a symlink
    // without a second round trip.
    match &link.obj_attributes {
        Nfs3Option::Some(attributes) => assert_eq!(attributes.type_, ftype3::NF3LNK),
        Nfs3Option::None => panic!("lookup did not return attributes for the symlink"),
    }
    let target = client
        .readlink(&READLINK3args {
            symlink: link.object,
        })
        .await
        .expect("readlink rpc")
        .unwrap();
    assert_eq!(target.data.0.as_ref(), b"hello.txt");

    // Descend a directory, which proves handles minted by lookup are usable as
    // directory handles too.
    let sub = client
        .lookup(&LOOKUP3args {
            what: diropargs3 {
                dir: root,
                name: name("sub"),
            },
        })
        .await
        .expect("lookup rpc")
        .unwrap()
        .object;
    let sub_listing = client
        .readdirplus(&READDIRPLUS3args {
            dir: sub,
            cookie: 0,
            cookieverf: Default::default(),
            dircount: 4096,
            maxcount: 65536,
        })
        .await
        .expect("readdirplus rpc")
        .unwrap();
    let sub_names: Vec<String> = sub_listing
        .reply
        .entries
        .0
        .iter()
        .map(|entry| String::from_utf8(entry.name.0.as_ref().to_vec()).expect("utf-8 name"))
        .collect();
    assert_eq!(sub_names, ["inner.txt"]);
}

#[tokio::test]
async fn test_nfs_readdirplus_resumes_from_a_cookie() {
    let test_repo = TestRepo::init();
    let mut builder = TestTreeBuilder::new(test_repo.repo.store().clone());
    for index in 0..6 {
        builder.file(repo_path(&format!("f{index}")), format!("{index}\n"));
    }
    let tree = builder.write_merged_tree();

    let mut connection = serve(snapshot_of(&tree)).await;
    let root = connection.root_nfs_fh3();
    let client = &mut connection.nfs3_client;

    let first = client
        .readdirplus(&READDIRPLUS3args {
            dir: root.clone(),
            cookie: 0,
            cookieverf: Default::default(),
            dircount: 4096,
            maxcount: 65536,
        })
        .await
        .expect("readdirplus rpc")
        .unwrap();
    let entries = &first.reply.entries.0;
    assert_eq!(entries.len(), 6);
    // Resume after the third entry. A client does this whenever a directory
    // does not fit one reply, and getting it wrong silently drops or repeats
    // files, which is why it is tested rather than assumed.
    let cookie = entries[2].cookie;
    let second = client
        .readdirplus(&READDIRPLUS3args {
            dir: root,
            cookie,
            cookieverf: Default::default(),
            dircount: 4096,
            maxcount: 65536,
        })
        .await
        .expect("readdirplus rpc")
        .unwrap();
    let resumed: Vec<String> = second
        .reply
        .entries
        .0
        .iter()
        .map(|entry| String::from_utf8(entry.name.0.as_ref().to_vec()).expect("utf-8 name"))
        .collect();
    assert_eq!(resumed, ["f3", "f4", "f5"]);
    assert!(second.reply.eof);
}

#[tokio::test]
async fn test_nfs_conflicted_file_is_a_readable_regular_file() {
    let test_repo = TestRepo::init();
    let mut builder = TestThreeWayMergeTreeBuilder::new(test_repo.repo.store().clone());
    builder.base().file(repo_path("f"), "base\n");
    builder.parent1().file(repo_path("f"), "left\n");
    builder.parent2().file(repo_path("f"), "right\n");
    let tree = builder.write_merged_tree();

    let mut connection = serve(snapshot_of(&tree)).await;
    let root = connection.root_nfs_fh3();
    let client = &mut connection.nfs3_client;

    let file = client
        .lookup(&LOOKUP3args {
            what: diropargs3 {
                dir: root,
                name: name("f"),
            },
        })
        .await
        .expect("lookup rpc")
        .unwrap()
        .object;
    let attributes = client
        .getattr(&GETATTR3args {
            object: file.clone(),
        })
        .await
        .expect("getattr rpc")
        .unwrap()
        .obj_attributes;
    assert_eq!(attributes.type_, ftype3::NF3REG);
    let read = client
        .read(&READ3args {
            file,
            offset: 0,
            count: 65536,
        })
        .await
        .expect("read rpc")
        .unwrap();
    // The size reported by getattr and the bytes returned by read have to agree
    // exactly, or a client truncates the conflict markers.
    assert_eq!(
        u64::try_from(read.data.as_ref().len()).unwrap(),
        attributes.size
    );
    let content = String::from_utf8(read.data.as_ref().to_vec()).expect("utf-8");
    assert!(
        content.contains("<<<<<<<"),
        "no conflict markers in:\n{content}"
    );
}

#[tokio::test]
async fn test_nfs_missing_name_and_wrong_type() {
    let test_repo = TestRepo::init();
    let mut builder = TestTreeBuilder::new(test_repo.repo.store().clone());
    builder.file(repo_path("f"), "x\n");
    let tree = builder.write_merged_tree();

    let mut connection = serve(snapshot_of(&tree)).await;
    let root = connection.root_nfs_fh3();
    let client = &mut connection.nfs3_client;

    let missing = client
        .lookup(&LOOKUP3args {
            what: diropargs3 {
                dir: root.clone(),
                name: name("nope"),
            },
        })
        .await
        .expect("lookup rpc");
    assert!(
        matches!(
            missing,
            nfs3_client::nfs3_types::nfs3::Nfs3Result::Err((nfsstat3::NFS3ERR_NOENT, _))
        ),
        "expected NFS3ERR_NOENT, got {missing:?}"
    );

    // readlink of a regular file is NFS3ERR_INVAL, per RFC 1813.
    let file = client
        .lookup(&LOOKUP3args {
            what: diropargs3 {
                dir: root,
                name: name("f"),
            },
        })
        .await
        .expect("lookup rpc")
        .unwrap()
        .object;
    let bad = client
        .readlink(&READLINK3args { symlink: file })
        .await
        .expect("readlink rpc");
    assert!(
        matches!(
            bad,
            nfs3_client::nfs3_types::nfs3::Nfs3Result::Err((nfsstat3::NFS3ERR_INVAL, _))
        ),
        "expected NFS3ERR_INVAL, got {bad:?}"
    );
}

#[tokio::test]
async fn test_nfs_stale_handle_is_rejected() {
    let test_repo = TestRepo::init();
    let mut builder = TestTreeBuilder::new(test_repo.repo.store().clone());
    builder.file(repo_path("f"), "x\n");
    let tree = builder.write_merged_tree();

    let mut connection = serve(snapshot_of(&tree)).await;
    let client = &mut connection.nfs3_client;

    // A handle carrying inode 1 but generation 0. The server stamps its startup
    // time into the first eight bytes of every handle, so a generation older
    // than the current one is exactly a handle left over from a previous mount.
    // This is what stops a restarted server from serving a client's cached
    // handles as if they still meant something.
    let mut data = 0u64.to_le_bytes().to_vec();
    data.extend_from_slice(&1u64.to_ne_bytes());
    let forged = nfs_fh3 {
        data: nfs3_client::nfs3_types::xdr_codec::Opaque::owned(data),
    };
    let result = client
        .getattr(&GETATTR3args { object: forged })
        .await
        .expect("getattr rpc");
    assert!(
        matches!(
            result,
            nfs3_client::nfs3_types::nfs3::Nfs3Result::Err((nfsstat3::NFS3ERR_STALE, _))
        ),
        "expected NFS3ERR_STALE, got {result:?}"
    );
}

#[tokio::test]
async fn test_nfs_dot_and_dotdot_resolve() {
    let test_repo = TestRepo::init();
    let mut builder = TestTreeBuilder::new(test_repo.repo.store().clone());
    builder.file(repo_path("sub/inner.txt"), "inner\n");
    let tree = builder.write_merged_tree();

    let mut connection = serve(snapshot_of(&tree)).await;
    let root = connection.root_nfs_fh3();
    let client = &mut connection.nfs3_client;

    let sub = client
        .lookup(&LOOKUP3args {
            what: diropargs3 {
                dir: root.clone(),
                name: name("sub"),
            },
        })
        .await
        .expect("lookup rpc")
        .unwrap()
        .object;

    // "." is the directory itself.
    let dot = client
        .lookup(&LOOKUP3args {
            what: diropargs3 {
                dir: sub.clone(),
                name: name("."),
            },
        })
        .await
        .expect("lookup rpc")
        .unwrap()
        .object;
    assert_eq!(dot.data.as_ref(), sub.data.as_ref());

    // ".." is the real parent. A kernel client that has lost a dentry asks for
    // this by name, and answering ENOENT makes `cd ..` fail for no visible
    // reason.
    let dotdot = client
        .lookup(&LOOKUP3args {
            what: diropargs3 {
                dir: sub,
                name: name(".."),
            },
        })
        .await
        .expect("lookup rpc")
        .unwrap()
        .object;
    assert_eq!(dotdot.data.as_ref(), root.data.as_ref());

    // And ".." from the root is the root.
    let root_dotdot = client
        .lookup(&LOOKUP3args {
            what: diropargs3 {
                dir: root.clone(),
                name: name(".."),
            },
        })
        .await
        .expect("lookup rpc")
        .unwrap()
        .object;
    assert_eq!(root_dotdot.data.as_ref(), root.data.as_ref());
}

/// Serves `tree` read-write on an ephemeral loopback port.
///
/// A separate helper from [`serve`] because the read-only bind wraps the
/// filesystem in an adapter that refuses the write procedures, which is exactly
/// what the read-only mount wants and exactly what this cannot use.
async fn serve_writable(tree: Arc<jj_vfs::OverlayTree>) -> TestConnection {
    let served = NfsTree::with_tree(tree, 0x6a6a_0002, 501, 20);
    let listener = NFSTcpListener::bind("127.0.0.1:0", served)
        .await
        .expect("bind a writable loopback NFS listener");
    let port = listener.get_listen_port();
    tokio::spawn(async move {
        drop(listener.handle_forever().await);
    });
    Nfs3ConnectionBuilder::new(TokioConnector, "127.0.0.1", "/")
        .connect_from_privileged_port(false)
        .mount_port(port)
        .nfs3_port(port)
        .mount()
        .await
        .expect("MOUNT and NFS handshake against our own writable server")
}

#[tokio::test]
async fn test_nfs_create_write_read_and_refuse_removing_a_tracked_file() {
    use nfs3_client::nfs3_types::nfs3::CREATE3args;
    use nfs3_client::nfs3_types::nfs3::REMOVE3args;
    use nfs3_client::nfs3_types::nfs3::RMDIR3args;
    use nfs3_client::nfs3_types::nfs3::WRITE3args;
    use nfs3_client::nfs3_types::nfs3::createhow3;
    use nfs3_client::nfs3_types::nfs3::sattr3;
    use nfs3_client::nfs3_types::nfs3::stable_how;

    let test_repo = TestRepo::init();
    let mut builder = TestTreeBuilder::new(test_repo.repo.store().clone());
    builder.file(repo_path("bun.lock"), "tracked\n");
    builder.file(repo_path("src/main.rs"), "fn main() {}\n");
    let merged = builder.write_merged_tree();
    let scratch = tempfile::tempdir().expect("a temporary directory");
    let lower = Arc::new(snapshot_of(&merged));
    let overlay = jj_vfs::Overlay::open(scratch.path().join("upper"), &lower.tree_key())
        .expect("open the writable layer");
    let tree = Arc::new(jj_vfs::OverlayTree::writable(lower, overlay));

    let mut connection = serve_writable(tree).await;
    let root = connection.root_nfs_fh3();
    let client = &mut connection.nfs3_client;

    // CREATE, which is what a client sends for the first `>` into a new name.
    let created = client
        .create(&CREATE3args {
            where_: diropargs3 {
                dir: root.clone(),
                name: name("installed.txt"),
            },
            how: createhow3::UNCHECKED(sattr3::default()),
        })
        .await
        .expect("create rpc")
        .unwrap();
    let file = match created.obj {
        Nfs3Option::Some(handle) => handle,
        Nfs3Option::None => panic!("create returned no file handle"),
    };

    let written = client
        .write(&WRITE3args {
            file: file.clone(),
            offset: 0,
            count: 9,
            stable: stable_how::FILE_SYNC,
            data: b"from bun\n".as_slice().into(),
        })
        .await
        .expect("write rpc")
        .unwrap();
    assert_eq!(written.count, 9);

    let read = client
        .read(&READ3args {
            file,
            offset: 0,
            count: 4096,
        })
        .await
        .expect("read rpc")
        .unwrap();
    assert_eq!(read.data.as_ref(), b"from bun\n");

    // A writable mount has to offer a write bit, or the kernel client refuses
    // the open before the server is ever asked.
    let listed = client
        .lookup(&LOOKUP3args {
            what: diropargs3 {
                dir: root.clone(),
                name: name("bun.lock"),
            },
        })
        .await
        .expect("lookup rpc")
        .unwrap();
    match &listed.obj_attributes {
        Nfs3Option::Some(attributes) => assert_eq!(attributes.mode, 0o644),
        Nfs3Option::None => panic!("lookup returned no attributes"),
    }

    // Deleting a tracked file: allowed, and the name has to stop being served
    // by the same RPC that served it a moment ago.
    let removed = client
        .remove(&REMOVE3args {
            object: diropargs3 {
                dir: root.clone(),
                name: name("bun.lock"),
            },
        })
        .await
        .expect("remove rpc");
    assert!(
        matches!(removed, nfs3_client::nfs3_types::nfs3::REMOVE3res::Ok(_)),
        "removing a tracked file over NFS returned {removed:?}"
    );
    let gone = client
        .lookup(&LOOKUP3args {
            what: diropargs3 {
                dir: root.clone(),
                name: name("bun.lock"),
            },
        })
        .await
        .expect("lookup rpc");
    match gone {
        nfs3_client::nfs3_types::nfs3::LOOKUP3res::Err((status, _)) => {
            assert_eq!(status, nfsstat3::NFS3ERR_NOENT);
        }
        nfs3_client::nfs3_types::nfs3::LOOKUP3res::Ok(_) => {
            panic!("a deleted tracked file still resolved over NFS")
        }
    }

    // Removing a tracked directory is where the line is, and the client has to
    // be told with an errno it can act on rather than a generic failure.
    let refused = client
        .rmdir(&RMDIR3args {
            object: diropargs3 {
                dir: root.clone(),
                name: name("src"),
            },
        })
        .await
        .expect("rmdir rpc");
    match refused {
        nfs3_client::nfs3_types::nfs3::RMDIR3res::Err((status, _)) => {
            assert_eq!(status, nfsstat3::NFS3ERR_ROFS);
        }
        nfs3_client::nfs3_types::nfs3::RMDIR3res::Ok(_) => {
            panic!("removing a tracked directory succeeded over NFS")
        }
    }

    // The listing agrees with the lookups: the deleted name is gone, the
    // refused one is still there.
    let listing = client
        .readdirplus(&READDIRPLUS3args {
            dir: root,
            cookie: 0,
            cookieverf: Default::default(),
            dircount: 4096,
            maxcount: 65536,
        })
        .await
        .expect("readdirplus rpc")
        .unwrap();
    let names: Vec<String> = listing
        .reply
        .entries
        .0
        .iter()
        .map(|entry| String::from_utf8(entry.name.0.as_ref().to_vec()).expect("utf-8 name"))
        .collect();
    assert_eq!(names, ["installed.txt", "src"]);
}

/// READDIRPLUS must inline attributes for entries it can size without reading
/// content, and omit them for the entries where sizing still means a read.
///
/// The distinction is the whole point of the optimization: a client that gets
/// attributes inline skips a LOOKUP per entry, which on a cold walk of a real
/// tree was 70% of every RPC made. Omitting them for the cases that would cost
/// a content read is what keeps listing a directory cheaper than reading it.
#[tokio::test]
async fn test_nfs_readdirplus_inlines_cheaply_sized_attributes() {
    let test_repo = TestRepo::init();
    let mut builder = TestTreeBuilder::new(test_repo.repo.store().clone());
    builder.file(repo_path("hello.txt"), "hello nfs\n");
    builder
        .file(repo_path("run.sh"), "#!/bin/sh\ntrue\n")
        .executable(true);
    builder.symlink(repo_path("link"), "hello.txt");
    builder.file(repo_path("sub/inner.txt"), "inner\n");
    let tree = builder.write_merged_tree();

    let mut connection = serve(snapshot_of(&tree)).await;
    let root = connection.root_nfs_fh3();
    let client = &mut connection.nfs3_client;

    let listing = client
        .readdirplus(&READDIRPLUS3args {
            dir: root,
            cookie: 0,
            cookieverf: Default::default(),
            dircount: 4096,
            maxcount: 65536,
        })
        .await
        .expect("readdirplus rpc")
        .unwrap();

    let mut seen: Vec<(String, Option<(ftype3, u64)>)> = listing
        .reply
        .entries
        .0
        .iter()
        .map(|entry| {
            let name = String::from_utf8(entry.name.0.as_ref().to_vec()).expect("utf-8 name");
            let attributes = match &entry.name_attributes {
                Nfs3Option::Some(attributes) => Some((attributes.type_, attributes.size)),
                Nfs3Option::None => None,
            };
            (name, attributes)
        })
        .collect();
    seen.sort_by(|a, b| a.0.cmp(&b.0));

    assert_eq!(
        seen,
        vec![
            // Sized from the git object header, so inlined.
            ("hello.txt".to_string(), Some((ftype3::NF3REG, 10))),
            // A symlink's length is only knowable by fetching its target, so it
            // keeps costing a LOOKUP rather than making every listing read.
            ("link".to_string(), None),
            ("run.sh".to_string(), Some((ftype3::NF3REG, 15))),
            // A directory reports a constant, so it is free to inline.
            ("sub".to_string(), Some((ftype3::NF3DIR, 4096))),
        ],
        "readdirplus must inline exactly the attributes it can compute without reading content"
    );
}

/// A conflicted path keeps its attributes out of READDIRPLUS.
///
/// This is the one case the pre-`file_size` reasoning still protects: a
/// conflict has no content until its sides are materialized into marker text,
/// so sizing it inside a directory listing would do exactly the expensive work
/// that inlining attributes is supposed to avoid.
#[tokio::test]
async fn test_nfs_readdirplus_omits_attributes_for_a_conflict() {
    let test_repo = TestRepo::init();
    let mut builder = TestThreeWayMergeTreeBuilder::new(test_repo.repo.store().clone());
    builder.base().file(repo_path("f"), "base\n");
    builder.parent1().file(repo_path("f"), "left\n");
    builder.parent2().file(repo_path("f"), "right\n");
    let tree = builder.write_merged_tree();

    let mut connection = serve(snapshot_of(&tree)).await;
    let root = connection.root_nfs_fh3();
    let client = &mut connection.nfs3_client;

    let listing = client
        .readdirplus(&READDIRPLUS3args {
            dir: root.clone(),
            cookie: 0,
            cookieverf: Default::default(),
            dircount: 4096,
            maxcount: 65536,
        })
        .await
        .expect("readdirplus rpc")
        .unwrap();
    let conflicted = listing
        .reply
        .entries
        .0
        .iter()
        .find(|entry| entry.name.0.as_ref() == b"f")
        .expect("the conflicted entry is listed");
    assert!(
        matches!(conflicted.name_attributes, Nfs3Option::None),
        "a conflicted entry must not be sized inside a listing"
    );

    // And it must still resolve by the route that does pay to materialize it,
    // so omitting the attributes costs a round trip and nothing else.
    let looked_up = client
        .lookup(&LOOKUP3args {
            what: diropargs3 {
                dir: root,
                name: name("f"),
            },
        })
        .await
        .expect("lookup rpc")
        .unwrap();
    match &looked_up.obj_attributes {
        Nfs3Option::Some(attributes) => {
            assert_eq!(attributes.type_, ftype3::NF3REG);
            assert!(attributes.size > 0, "a materialized conflict has content");
        }
        Nfs3Option::None => panic!("lookup must still size a conflicted path"),
    }
}
