#![cfg(windows)]

#[path = "../src/control/windows.rs"]
mod windows;

use std::thread;
use std::time::{Duration, Instant};
use wholeseek_core::control::{Caller, Operation, Request, Response, transport};
use windows::Endpoint;

#[test]
fn owner_only_pipe_round_trip() {
    let mut endpoint = Endpoint::new(&format!("test-{}", std::process::id())).unwrap();
    let mut listener = endpoint.bind().unwrap();
    assert!(
        endpoint.bind().is_err(),
        "the first instance must own the endpoint"
    );
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(mut client) = listener.accept().unwrap() {
                transport::serve(&mut client, |req| {
                    assert_eq!(req.command, Operation::Status);
                    Response::success(serde_json::json!({"connected": true}))
                })
                .unwrap();
                return;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
    });
    let reply = endpoint.call(&Request::new(Operation::Status)).unwrap();
    assert!(reply.ok);
    assert_eq!(reply.data.unwrap()["connected"], true);
    server.join().unwrap();
}
