use funding_arb_domain::TradingMode;
use std::{env, process::ExitCode, str::FromStr, thread, time::Duration};

fn main() -> ExitCode {
    let mode = env::var("SIGNEX_MODE").unwrap_or_else(|_| String::from("PAPER"));
    if TradingMode::from_str(&mode) != Ok(TradingMode::Paper) {
        eprintln!("M0 engine refuses every mode except PAPER");
        return ExitCode::FAILURE;
    }
    println!("Signex M0 engine supervisor started; trading capability is BLOCKED");
    loop {
        thread::sleep(Duration::from_secs(60));
    }
}
