//! Local transport. One request and one reply per connection.

use super::{
    ErrorCode, Failure, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES, Request, Response, VERSION,
    read_frame, read_request, write_frame,
};
use std::io::{self, BufReader, Read, Write};

pub fn exchange(stream: &mut (impl Read + Write), request: &Request) -> io::Result<Response> {
    write_frame(stream, request, MAX_REQUEST_BYTES)?;
    let frame = read_frame(&mut BufReader::new(stream), MAX_RESPONSE_BYTES)?;
    let response: Response = serde_json::from_slice(&frame)?;
    let shape_matches = match (&response.data, &response.error) {
        (Some(_), None) => response.ok,
        (None, Some(_)) => !response.ok,
        _ => false,
    };
    if response.v != VERSION || !shape_matches {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid control response",
        ));
    }
    Ok(response)
}

pub fn serve(
    stream: &mut (impl Read + Write),
    handler: impl FnOnce(Request) -> Response,
) -> io::Result<()> {
    let response = match read_request(&mut BufReader::new(&mut *stream)) {
        Ok(request) => handler(request),
        Err(error) => Response::failure(error),
    };
    match write_frame(stream, &response, MAX_RESPONSE_BYTES) {
        Err(e) if e.kind() == io::ErrorKind::InvalidData => write_frame(
            stream,
            &Response::failure(Failure::new(
                ErrorCode::Internal,
                "control response exceeded size limit",
            )),
            MAX_RESPONSE_BYTES,
        ),
        result => result,
    }
}
