mod agent;
mod cli;
mod config;
mod model;
mod tools;
mod ui;

#[tokio::main]
async fn main() {
    let code = match cli::run(std::env::args().skip(1).collect()).await {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err:#}");
            1
        }
    };
    std::process::exit(code);
}
