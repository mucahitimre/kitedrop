//! Regenerates `protocol/vectors/*.json`: `cargo run -p kitedrop-core --example gen_vectors`.

#[path = "../tests/support/vectors.rs"]
mod vectors;

fn main() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../protocol/vectors");
    std::fs::create_dir_all(&dir).unwrap();
    for (name, body) in vectors::generate() {
        std::fs::write(dir.join(name), body).unwrap();
        println!("wrote protocol/vectors/{name}");
    }
}
