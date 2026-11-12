//! Save a Soulseek login the way the app does (username → config.toml, password → keyring),
//! so `goalseek` starts signed in. Reads GOALSEEK_USER / GOALSEEK_PASS from the environment;
//! never prints the password.
//!
//! ```sh
//! GOALSEEK_USER=… GOALSEEK_PASS=… cargo run -p goalseek-core --example save_login
//! ```

use goalseek_core::{Credentials, config};

fn main() {
    let (Ok(username), Ok(password)) = (
        std::env::var("GOALSEEK_USER"),
        std::env::var("GOALSEEK_PASS"),
    ) else {
        eprintln!("GOALSEEK_USER and GOALSEEK_PASS must be set");
        std::process::exit(2);
    };
    let credentials = Credentials { username, password };
    match config::save_credentials(&credentials) {
        Ok(()) => println!(
            "saved login for {} (password in keyring) to {}",
            credentials.username,
            config::config_dir().display()
        ),
        Err(e) => {
            eprintln!("keyring unavailable: {e:?}");
            std::process::exit(1);
        }
    }
}
