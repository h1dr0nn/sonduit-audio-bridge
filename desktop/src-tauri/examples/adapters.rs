//! Print the network adapters Sonduit would consider, best first.
//!
//! Run with `cargo run -p sonduit-desktop --example adapters`. With a phone
//! tethered over USB its adapter should come first; without one, the list
//! should still be the machine's real interfaces rather than empty.

fn main() {
    match sonduit_desktop::bridge::adapters::enumerate() {
        Ok(adapters) if adapters.is_empty() => {
            println!("no adapters with a gateway");
        }
        Ok(adapters) => {
            for (rank, adapter) in adapters.iter().enumerate() {
                let tether =
                    if sonduit_desktop::bridge::adapters::looks_like_tether(&adapter.description) {
                        "tether"
                    } else {
                        "      "
                    };
                println!(
                    "{rank}. [{tether}] {}\n     local {}  gateway {}",
                    adapter.description, adapter.local, adapter.gateway
                );
            }
        }
        Err(error) => println!("enumerate failed: {error}"),
    }
}
