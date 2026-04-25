fn main() {
    if let Err(error) = acli::run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}
