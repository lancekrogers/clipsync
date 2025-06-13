//! Example program to generate a sample ClipSync configuration file

use clipsync::config::Config;

fn main() {
    println!("Generating example ClipSync configuration...\n");

    let example = Config::generate_example();
    println!("{}", example);

    println!("\n\nSave this to ~/.config/clipsync/config.toml to use it.");
}
