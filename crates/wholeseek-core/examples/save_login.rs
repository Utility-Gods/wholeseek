//! Save a Soulseek login the way the app does (username → config.toml, password → keyring),
//! so `wholeseek` starts signed in. Reads WHOLESEEK_USER / WHOLESEEK_PASS from the environment;
//! never prints the password.
//!
//! ```sh
//! WHOLESEEK_USER=… WHOLESEEK_PASS=… cargo run -p wholeseek-core --example save_login
//! ```

use wholeseek_core::{Credentials, config};

fn main() {
    let (Ok(username), Ok(password)) = (
        std::env::var("WHOLESEEK_USER"),
        std::env::var("WHOLESEEK_PASS"),
    ) else {
        eprintln!("WHOLESEEK_USER and WHOLESEEK_PASS must be set");
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
