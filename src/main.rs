#![recursion_limit = "256"]

#[tokio::main]
async fn main() {
    let code = match oy::run(std::env::args().skip(1).collect()).await {
        Ok(code) => code,
        Err(err) => {
            oy::err_line(format_args!("error: {err:#}"));
            1
        }
    };
    std::process::exit(code);
}
