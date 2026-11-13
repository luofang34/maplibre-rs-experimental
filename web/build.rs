use std::{env, fs, path::Path};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = env::var("OUT_DIR")?;

    println!("cargo:rerun-if-changed=./flatbuffer");

    let mut flatbuffer = Vec::new();
    for entry in fs::read_dir("./flatbuffer")? {
        flatbuffer.push(entry?.path());
    }

    flatc_rust::run(flatc_rust::Args {
        inputs: &flatbuffer
            .iter()
            .map(|buf| buf.as_path())
            .collect::<Vec<_>>(),
        out_dir: Path::new(&out_dir),
        extra: &[
            "--include-prefix",
            "platform::singlethreaded::transferables",
        ],
        ..Default::default()
    })?;
    Ok(())
}
