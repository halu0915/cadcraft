//! CLI over [`cadcraft_dwg::dynblock_author`].
//!
//! `cargo run -p cadcraft-dwg --example dynblock_author -- flip <in.dwg> <BLOCK> <out.dwg>`

use acadrust::types::{Vector2, Vector3};
use cadcraft_dwg::dynblock_author::{self, parse_visibility_states, read_dwg, write_dwg};

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let a: Vec<String> = std::env::args().collect();
    let (cmd, input, block, out) = match a.as_slice() {
        [_, c, i, b, o, ..] => (c.as_str(), i, b, o),
        _ => return Err("usage: flip|vis|width <in.dwg> <BLOCK> <out.dwg> [vis: NAME=i,j;...]".into()),
    };
    let bytes = std::fs::read(input).map_err(|e| e.to_string())?;
    let out_bytes = match cmd {
        "flip" => dynblock_author::flip_dwg_bytes(&bytes, block, [15.0, -3.0], [15.0, 33.0])?,
        "vis" => {
            let spec = a.get(5).ok_or("vis needs a state list")?;
            dynblock_author::visibility_dwg_bytes(
                &bytes,
                block,
                &parse_visibility_states(spec)?,
                [33.0, 30.0],
            )?
        }
        "width" => {
            let mut doc = read_dwg(&bytes)?;
            dynblock_author::add_linear_stretch(
                &mut doc,
                block,
                "Width",
                Vector3::new(0.0, 0.0, 0.0),
                Vector3::new(60.0, 0.0, 0.0),
                [Vector2::new(70.0, 45.0), Vector2::new(50.0, -5.0)],
                &[(0, vec![1, 2])],
                &[60.0, 80.0, 100.0, 120.0],
            )?;
            write_dwg(&doc)?
        }
        _ => return Err(format!("unknown command {cmd}")),
    };
    std::fs::write(out, out_bytes).map_err(|e| e.to_string())?;
    println!("wrote {out}");
    Ok(())
}