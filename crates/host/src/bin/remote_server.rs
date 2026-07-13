fn main() {
    if let Err(error) = host::serve_remote_server_cli() {
        eprintln!("shirushi-remote-server: {error:#}");
        std::process::exit(1);
    }
}
