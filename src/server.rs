use std::str;
use std::sync::{Arc, Mutex};
use std::io::{Result as IoResult, Error as IoError, ErrorKind as IoErrorKind};

use tiny_http::{Server as TinyServer, Method, Header, Request, Response, StatusCode};

use crate::core::Model;

pub struct Server<'a> {
    model: Arc::<Mutex::<Model<'a>>>
}

impl<'a> Server<'a> {
    #[inline]
    pub fn new(model: Arc::<Mutex::<Model<'a>>>) -> Self {
        Self {model}
    }

    pub fn serve(&mut self, addr: &'a str) -> IoResult::<()> {
        let server = TinyServer::http(addr).map_err(|err| {
            return IoError::new(IoErrorKind::AddrNotAvailable, format!("could not serve at `{addr}`: {err}"))
        }).unwrap();

        println!("listening on <http://{addr}/>");

        for rq in server.incoming_requests() {
            match (rq.method(), rq.url()) {
                (Method::Post, "/api/search") => self.serve_search(rq)?,
                (Method::Get, "/script.js") => serve_bytes(rq, include_bytes!("script.js"), "text/javascript; charset=UTF-8")?,
                _ => serve_bytes(rq, include_bytes!("query.html"), "text/html; charset=UTF-8")?
            }
        }

        Ok(())
    }

    pub fn serve_search(&self, mut request: Request) -> IoResult::<()> {
        let mut buf = Vec::new();
        if let Err(err) = request.as_reader().read_to_end(&mut buf) {
            eprintln!("ERROR: could not read the body of the request: {err}");
            return serve_500(request);
        }

        let body = match str::from_utf8(&buf) {
            Ok(body) => body,
            Err(err) => {
                eprintln!("could not interpret body as UTF-8 string: {err}");
                return serve_400(request, "Body must be a valid UTF-8 string")
            }
        };

        let model = self.model.lock().unwrap();
        let result = model.search(&body);

        let json = match serde_json::to_string(&result.iter().take(20).collect::<Vec<_>>()) {
            Ok(json) => json,
            Err(err) => {
                eprintln!("could not convert search results to JSON: {err}");
                return serve_500(request)
            }
        };

        let content_type_header = Header::from_bytes("Content-Type", "application/json").unwrap();
        request.respond(Response::from_string(&json).with_header(content_type_header))
    }
}

#[inline]
fn serve_400(request: Request, message: &str) -> IoResult::<()> {
    request.respond(Response::from_string(format!("400: {message}")).with_status_code(StatusCode(400)))
}

#[inline]
fn serve_500(request: Request) -> IoResult::<()> {
    request.respond(Response::from_string("500").with_status_code(StatusCode(500)))
}

#[inline]
fn serve_bytes(request: Request, bytes: &[u8], content_type: &str) -> IoResult::<()> {
    let content_type_header = Header::from_bytes("Content-Type", content_type).unwrap();
    request.respond(Response::from_data(bytes).with_header(content_type_header))
}
