use std::{thread, time::Duration};

fn main() {
    println!("Perpeto M0 reconciler started; no venue accounts are configured");
    loop {
        thread::sleep(Duration::from_secs(60));
    }
}
