use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    host_common::run("stone-raft host-wsl")
}
