#[path = "../native_fetch.rs"]
mod native_fetch;

fn main() {
    match native_fetch::run() {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("showy-quota-fetch: {error}");
            std::process::exit(1);
        }
    }
}
