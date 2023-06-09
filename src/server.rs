use crate::communication::request::{Request, StaticRequestData};

use crate::communication::response::Response;
use crate::communication::router::Router;
use crate::ds::trie::Trie;
use crate::log;
use crate::log::logger::Logger;
use crate::utils::file::get_first_html_file_name;
use crate::utils::general::is_static_file;
use crate::utils::guess::guess_mime_type;
use crate::utils::thread_pool::ThreadPool;

use std::env::current_dir;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::fs::read_to_string;
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex};

// This is the main entry point for the server.
pub struct Server {
    thread_pool: ThreadPool,
    listener: TcpListener,
    routers: Arc<Mutex<Trie<Router>>>,
    _address: String,
    root_path: String,
}

// Represents the server address, which is a tuple of an IP address and a port.
struct Address {
    ip: (u8, u8, u8, u8),
    port: u16,
}

impl Display for Address {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}.{}.{}.{}:{}",
            self.ip.0, self.ip.1, self.ip.2, self.ip.3, self.port
        )
    }
}

impl Server {
    pub fn new(ip: (u8, u8, u8, u8), port: u16) -> Result<Server, Box<dyn Error>> {
        let _address: String = Address { ip, port }.to_string();
        match TcpListener::bind(&_address) {
            Ok(listener) => {
                Logger::init("log.txt");
                log!("Server listening on: {}", _address);
                return Ok(Server {
                    thread_pool: ThreadPool::new(5),
                    listener,
                    routers: Arc::new(Mutex::new(Trie::new())),
                    _address,
                    root_path: current_dir().unwrap_or_default().display().to_string(),
                });
            }
            Err(e) => {
                log!("Listener Error: {:#?}", e);
                return Err(Box::new(e));
            }
        };
    }

    pub fn run(&mut self) -> Result<(), Box<dyn Error>> {
        for stream in self.listener.incoming() {
            let routers = self.routers.clone();
            match stream {
                Ok(stream) => {
                    self.thread_pool
                        .execute(move || match Request::build_request(&stream) {
                            Ok(mut request) => {
                                Self::handle_loop(&routers, &stream, &mut request);
                            }
                            Err(e) => log!("Request Error: {:#?}", e),
                        });
                }
                Err(e) => {
                    log!("Stream Error: {:#?}", e);
                    return Err(Box::new(e));
                }
            }
        }
        Ok(())
    }

    // Execute main request-response "loop" logic for the server.
    fn handle_loop(routers: &Arc<Mutex<Trie<Router>>>, stream: &TcpStream, request: &mut Request) {
        Self::check_static_request(request);
        let mut response = Response::new();
        Self::match_router(routers, request, &mut response);
        log!("Request: {:#?}", request);
        if let Err(e) = response.send(stream) {
            log!("Response Error: {:#?}", e);
        }
    }

    // Check if the request is for a static file, and add the static request data to the request object if so. Also change to forward to the static route.
    fn check_static_request(request: &mut Request) {
        if request.path == "/" {
            request.static_request_data = Some(StaticRequestData { path: None });
            request.path = String::from("/static");
        } else {
            let static_info = Self::is_static_path(request);
            if static_info.0 {
                request.static_request_data = Some(StaticRequestData {
                    path: Some(static_info.1),
                });
                request.path = String::from("/static");
            }
        }
    }

    // Static method to check if the request path is a static file.
    fn is_static_path(request: &Request) -> (bool, String) {
        let mut is_static = false;
        let path = Path::new(&request.path);
        match path.extension() {
            Some(extension) => {
                is_static = is_static_file(&extension.to_string_lossy());
            }
            None => {}
        }
        (is_static, path.to_string_lossy().to_string())
    }

    // Static method to match the request to the correct route, and then call the possible user registered function found on that route.
    fn match_router(
        routers: &Arc<Mutex<Trie<Router>>>,
        request: &mut Request,
        response: &mut Response,
    ) {
        routers.lock().unwrap().search(&request.path).map(|router| {
            router.execute_middleware(request);
            router
                .find_route(request)
                .and_then(|route| route.method_map.get(&request.method).cloned())
                .map(|func| (func)(request, response));
        });
    }

    // Register a router with the server. Routers are used to group routes together.
    pub fn router(&mut self, router: Router) {
        self.routers
            .lock()
            .unwrap()
            .insert(router.base_path.clone().as_str(), router);
    }

    // Register a route with the server that serves static files from a directory starting from project root.
    pub fn serve_static(&mut self, dir: &str) {
        let root_path = format!("{}\\{}", self.root_path, dir);
        let mut router = Router::new("/static");
        router.route("", "GET", move |request, response| {
            if let Some((path, extension)) = Self::get_static_file_details(request, &root_path) {
                match read_to_string(path) {
                    Ok(file_content) => {
                        response.set_contents(&guess_mime_type(&extension), &file_content)
                    }
                    Err(e) => {
                        response.set_status(404, "Not Found");
                        log!("File Read Error: {:#?}", e);
                    }
                }
            }
        });
        self.router(router);
    }

    fn get_static_file_details(request: &Request, root_path: &str) -> Option<(String, String)> {
        if let Some(ref data) = request.static_request_data {
            // If the request has a path, use that path to get the file. Otherwise, get the first HTML file in the directory.
            if let Some(ref path) = data.path {
                return Some((
                    format!("{}\\{}", root_path, path),
                    path.split('.').last().unwrap_or("text/plain").to_string(),
                ));
            } else {
                match get_first_html_file_name(Path::new(&root_path)) {
                    Ok((resource, extension)) => {
                        return Some((format!("{}\\{}", root_path, resource), extension));
                    }
                    Err(e) => {
                        log!("Static File Retrieval Error (No HTML File Found): {:#?}", e);
                        return None;
                    }
                }
            }
        }
        None
    }
}
