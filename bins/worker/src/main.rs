use std::{thread, time::Duration};

fn main() {
    println!("Perpeto M0 worker started; no outbox handlers are registered");
    loop {
        thread::sleep(Duration::from_secs(60));
    }
}
