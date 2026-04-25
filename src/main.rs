macro_rules! println {
    () => {
        $crate::highlight::stdout("\n")
    };
    ($($arg:tt)*) => {{
        $crate::highlight::stdout(&format!("{}\n", format_args!($($arg)*)))
    }};
}

macro_rules! eprintln {
    () => {
        $crate::highlight::stderr("\n")
    };
    ($($arg:tt)*) => {{
        $crate::highlight::stderr(&format!("{}\n", format_args!($($arg)*)))
    }};
}

mod agent;
mod cli;
mod config;
mod highlight;
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
